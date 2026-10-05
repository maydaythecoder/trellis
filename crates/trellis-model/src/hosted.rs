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
use crate::runtime::RuntimeError;
use crate::secret::ApiKey;
use crate::transport::{self, Auth, Endpoint, Proxy, SendError};
use serde_json::{Map, Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    let thinking = thinking_sent(protocol, thinking, every_turn_replayed(conv));
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
            let tokens = v["usage"]["completion_tokens"].as_u64();
            // Already counts cached prompt tokens; Anthropic's figure below is
            // summed to match it.
            let prompt_tokens = v["usage"]["prompt_tokens"].as_u64();
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
                prompt_tokens,
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
                prompt_tokens,
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
            && (body.contains("context_length_exceeded") || body.contains("prompt is too long")))
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
/// `Retry-After` in seconds or as an IMF-fixdate HTTP date
/// (`Sun, 06 Nov 1994 08:49:37 GMT`) when present, capped; a date at or
/// before now waits zero. Otherwise 1, 2, 4 s. The obsolete RFC 850 and
/// asctime date forms are not parsed and fall back to the fixed backoff
/// (ADR 0024 §4).
pub fn retry_delay(retry_after: Option<&str>, attempt: u32) -> Duration {
    retry_delay_at(retry_after, attempt, SystemTime::now())
}

/// `retry_delay` against a given clock, so the date form is testable
/// without depending on when the test runs.
fn retry_delay_at(retry_after: Option<&str>, attempt: u32, now: SystemTime) -> Duration {
    let backoff = Duration::from_secs(1u64 << attempt.saturating_sub(1).min(5));
    let Some(v) = retry_after.map(str::trim) else {
        return backoff;
    };
    if let Ok(s) = v.parse::<f64>() {
        return if s.is_finite() && s >= 0.0 {
            // Clamped before converting: `from_secs_f64` panics past u64::MAX
            // seconds, and the value is whatever the provider sent.
            Duration::from_secs_f64(s.min(MAX_RETRY_WAIT.as_secs_f64()))
        } else {
            backoff
        };
    }
    let Some(at) = imf_fixdate_unix_secs(v) else {
        return backoff;
    };
    // A date before 1970 is in the past for any clock this runs on.
    let Ok(at) = u64::try_from(at) else {
        return Duration::ZERO;
    };
    let now = now.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    Duration::from_secs(at)
        .saturating_sub(now)
        .min(MAX_RETRY_WAIT)
}

/// Seconds since the Unix epoch of an IMF-fixdate (RFC 9110 §5.6.7), the one
/// form senders must use. Parsed by hand rather than adding a dependency for
/// one header.
///
/// Anything else is `None`: the obsolete RFC 850 and asctime forms, a zone
/// other than `GMT`, a day that does not exist in its month (`31 Feb`), or an
/// out-of-range time. The day name must be a day name but is not checked
/// against the date; the wait is computed from the date.
fn imf_fixdate_unix_secs(v: &str) -> Option<i64> {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // `Sun, 06 Nov 1994 08:49:37 GMT` is fixed width, so every field sits at
    // a fixed offset and any deviation fails a check below.
    let b = v.as_bytes();
    if b.len() != 29
        || !DAYS.iter().any(|d| d.as_bytes() == &b[0..3])
        || &b[3..5] != b", "
        || b[7] != b' '
        || b[11] != b' '
        || b[16] != b' '
        || b[19] != b':'
        || b[22] != b':'
        || &b[25..29] != b" GMT"
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let d = &b[r];
        d.iter()
            .all(u8::is_ascii_digit)
            .then(|| d.iter().fold(0i64, |n, c| n * 10 + i64::from(c - b'0')))
    };
    let day = num(5..7)?;
    let month = MONTHS.iter().position(|m| m.as_bytes() == &b[8..11])? as i64 + 1;
    let year = num(12..16)?;
    let (hour, minute, second) = (num(17..19)?, num(20..22)?, num(23..25)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_len = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    // RFC 9110's grammar allows second 60, a leap second.
    if !(1..=month_len).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days from 1970-01-01 to a proleptic Gregorian date (H. Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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
        // Missing: the fixed backoff.
        assert_eq!(retry_delay(None, 1), Duration::from_secs(1));
        assert_eq!(retry_delay(None, 3), Duration::from_secs(4));
        // An HTTP date: the time until it, against a fixed clock.
        assert_eq!(
            retry_delay_at(
                Some("Wed, 21 Oct 2026 07:28:00 GMT"),
                2,
                unix(1_792_567_650)
            ),
            Duration::from_secs(30)
        );
        // Negative or infinite seconds: the backoff, not a panic in
        // `Duration::from_secs_f64` and not the cap.
        for v in ["-1", "inf"] {
            assert_eq!(retry_delay(Some(v), 2), Duration::from_secs(2), "{v}");
        }
    }

    /// A fixed clock: the date tests must not depend on when they run.
    fn unix(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn an_http_date_in_the_future_waits_the_difference() {
        // Sun, 06 Nov 1994 08:49:37 GMT is 784111777 s after the epoch.
        let v = Some("Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(
            retry_delay_at(v, 1, unix(784_111_777 - 42)),
            Duration::from_secs(42)
        );
        // A leap day, so a wrong month length would shift the answer.
        assert_eq!(
            imf_fixdate_unix_secs("Thu, 29 Feb 2024 00:00:00 GMT"),
            Some(1_709_164_800)
        );
        // 2000 is a leap year by the 400 rule, and second 60 is a leap
        // second RFC 9110 allows.
        assert_eq!(
            imf_fixdate_unix_secs("Tue, 29 Feb 2000 00:00:00 GMT"),
            Some(951_782_400)
        );
        assert_eq!(
            imf_fixdate_unix_secs("Sun, 06 Nov 1994 08:49:60 GMT"),
            Some(784_111_800)
        );
    }

    #[test]
    fn an_http_date_now_or_past_waits_zero_not_the_backoff() {
        // A date already reached says "retry now", as `Retry-After: 0` does.
        let v = Some("Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(retry_delay_at(v, 3, unix(784_111_777)), Duration::ZERO);
        assert_eq!(retry_delay_at(v, 3, unix(1_800_000_000)), Duration::ZERO);
        // Before the epoch: negative seconds, still the past, not a wrap.
        assert_eq!(
            retry_delay_at(Some("Mon, 01 Jan 1900 00:00:00 GMT"), 3, unix(0)),
            Duration::ZERO
        );
    }

    #[test]
    fn an_http_date_beyond_the_cap_waits_max_retry_wait() {
        // A date is as untrusted as a number of seconds: a provider naming a
        // far-off date must not park the run until then.
        assert_eq!(
            retry_delay_at(Some("Fri, 31 Dec 9999 23:59:59 GMT"), 1, unix(0)),
            MAX_RETRY_WAIT
        );
        let v = Some("Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(retry_delay_at(v, 1, unix(784_111_777 - 61)), MAX_RETRY_WAIT);
    }

    #[test]
    fn a_malformed_or_obsolete_http_date_falls_back_to_the_backoff() {
        // Each names a future instant if misparsed, so a lenient parser shows
        // up as a wait other than the 2 s backoff.
        let now = unix(0);
        for v in [
            // RFC 850 and asctime: obsolete, and not parsed.
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
            // Days that do not exist.
            "Tue, 31 Feb 2026 00:00:00 GMT",
            "Sat, 29 Feb 2025 00:00:00 GMT",
            "Mon, 00 Jan 2026 00:00:00 GMT",
            "Fri, 31 Apr 2026 00:00:00 GMT",
            // 1900 is not a leap year: divisible by 100 and not by 400.
            "Thu, 29 Feb 1900 00:00:00 GMT",
            // A zone other than GMT.
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 06 Nov 1994 08:49:37 +0000",
            // Out-of-range time, unknown names, wrong widths, garbage.
            "Sun, 06 Nov 1994 24:00:00 GMT",
            "Sun, 06 Nov 1994 08:60:00 GMT",
            "Sun, 06 Nov 1994 08:49:61 GMT",
            "Xyz, 06 Nov 1994 08:49:37 GMT",
            "Sun, 06 Noz 1994 08:49:37 GMT",
            "Sun, 6 Nov 1994 08:49:37 GMT",
            "Sun, +6 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 08:49:37 GMTX",
            "Sun, 06 Nov 1994 08:49:37 gmt",
            // One separator wrong at a time, the rest a valid date.
            "Sun, 06-Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov-1994 08:49:37 GMT",
            "Sun, 06 Nov 1994T08:49:37 GMT",
            "Sun, 06 Nov 1994 08.49:37 GMT",
            "Sun, 06 Nov 1994 08:49.37 GMT",
            "soon",
            "",
        ] {
            assert_eq!(
                retry_delay_at(Some(v), 2, now),
                Duration::from_secs(2),
                "{v:?}"
            );
        }
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
    fn a_retry_after_too_large_for_a_duration_is_capped_not_a_panic() {
        // `Duration::from_secs_f64` panics above u64::MAX seconds, and the
        // cap was applied after it: a provider's `1e20` panicked the run
        // mid-flight, before rollback.
        for v in ["1e20", "1.8446744073709552e19", "1e308"] {
            assert_eq!(retry_delay(Some(v), 1), MAX_RETRY_WAIT, "{v}");
        }
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
        // OpenAI sends this text beside the `context_length_exceeded` code,
        // which is the documented marker; the text alone is not one.
        assert!(!is_context_overflow(
            400,
            r#"{"error":{"message":"This model's maximum context length is 8192 tokens"}}"#
        ));
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
