//! An OpenAI-compatible streaming client.
//!
//! **Built against the shape, not against a vendor.** Ollama serves
//! `http://localhost:11434/v1` and llama.cpp's server speaks the same
//! protocol, so moving between them — which is what you do the moment you
//! need grammar-constrained decoding, since Ollama does not expose it — is a
//! change of base URL and nothing else.
//!
//! Streaming is server-sent events: `data: {json}` lines, terminated by
//! `data: [DONE]`. Parsed incrementally, because the whole reason to stream
//! is to show the first token before the last one exists.

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Where the model is, and what to ask for.
#[derive(Debug, Clone)]
pub struct Config {
    /// e.g. `http://localhost:11434/v1`
    pub base_url: String,
    pub model: String,
    /// Ollama ignores this; llama.cpp and hosted endpoints do not. Sent only
    /// when set, so a server that rejects unknown fields still works.
    pub api_key: Option<String>,
    pub request_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/v1".into()),
            model: std::env::var("CHAT_MODEL").unwrap_or_else(|_| "qwen3:8b".into()),
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            // Generous: a 9B model on a busy 16GB machine can take seconds
            // to first token, and a timeout shorter than that reads as the
            // server being down.
            request_timeout: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "user".into(),
            content: content.into(),
        }
    }
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage {
            role: "system".into(),
            content: content.into(),
        }
    }
}

/// A tool, in the OpenAI function-calling shape that Ollama's endpoint takes.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: Function,
}

#[derive(Debug, Clone, Serialize)]
pub struct Function {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl Tool {
    pub fn new(name: &str, description: &str, parameters: serde_json::Value) -> Self {
        Tool {
            kind: "function",
            function: Function {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct Request<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [Tool]>,
}

/// One piece of a streamed response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk {
    Token(String),
    /// A tool the model asked for. Carried separately from the text, which
    /// is the distinction that matters — see `tool_calls_of`.
    ToolCall {
        name: String,
        arguments: String,
    },
    Done,
}

/// What came back from one non-streamed request, for the smoke test.
#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub content: String,
    pub tool_calls: Vec<(String, String)>,
}

impl Completion {
    /// Whether the model used the tool channel rather than describing a call
    /// in prose.
    ///
    /// This is the measurement A.7 asks for, and the reason it is a method
    /// rather than a grep: JSON inside `content` is the *failure* — it is
    /// what mistral-nemo does about 70% of the time — and a check that
    /// searched the whole body for a brace would score that as a success.
    pub fn used_tool_channel(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

#[derive(Debug, Deserialize)]
struct StreamResponse {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: Delta,
}

#[derive(Debug, Default, Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BlockingResponse {
    #[serde(default)]
    choices: Vec<BlockingChoice>,
}

#[derive(Debug, Deserialize)]
struct BlockingChoice {
    #[serde(default)]
    message: BlockingMessage,
}

#[derive(Debug, Default, Deserialize)]
struct BlockingMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<BlockingToolCall>,
}

#[derive(Debug, Deserialize)]
struct BlockingToolCall {
    #[serde(default)]
    function: Option<BlockingFunction>,
}

#[derive(Debug, Deserialize)]
struct BlockingFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

pub struct Client {
    http: reqwest::Client,
    config: Config,
}

impl Client {
    pub fn new(config: Config) -> reqwest::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()?;
        Ok(Client { http, config })
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    fn post(&self, body: &Request<'_>) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.config.base_url))
            .json(body);
        if let Some(key) = &self.config.api_key {
            req = req.bearer_auth(key);
        }
        req
    }

    /// Streams a completion, sending each chunk to `out`.
    ///
    /// Bounded channel by contract: the caller passes the one the event loop
    /// reads, and a model that outruns the renderer must apply backpressure
    /// rather than grow memory.
    pub async fn stream(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        out: tokio::sync::mpsc::Sender<Chunk>,
    ) -> Result<(), String> {
        let body = Request {
            model: &self.config.model,
            messages,
            stream: true,
            tools,
        };
        let resp = self
            .post(&body)
            .send()
            .await
            .map_err(|e| format!("cannot reach {}: {e}", self.config.base_url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("{status}: {}", text.trim()));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(part) = stream.next().await {
            let bytes = part.map_err(|e| format!("stream broke: {e}"))?;
            buf.push_str(&String::from_utf8_lossy(&bytes));
            // Events are newline-delimited; a chunk boundary can land mid
            // line, so only whole lines are consumed and the tail is kept.
            while let Some(nl) = buf.find('\n') {
                let line: String = buf.drain(..=nl).collect();
                match parse_sse_line(line.trim_end()) {
                    Some(Chunk::Done) => {
                        let _ = out.send(Chunk::Done).await;
                        return Ok(());
                    }
                    Some(chunk) => {
                        if out.send(chunk).await.is_err() {
                            // The interface has gone. Not an error.
                            return Ok(());
                        }
                    }
                    None => {}
                }
            }
        }
        let _ = out.send(Chunk::Done).await;
        Ok(())
    }

    /// One non-streamed completion. Used by the tool-calling smoke test,
    /// where the thing being measured is the shape of the whole response.
    pub async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
    ) -> Result<Completion, String> {
        let body = Request {
            model: &self.config.model,
            messages,
            stream: false,
            tools,
        };
        let resp = self
            .post(&body)
            .send()
            .await
            .map_err(|e| format!("cannot reach {}: {e}", self.config.base_url))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("{status}: {}", text.trim()));
        }
        parse_completion(&text)
    }
}

/// Parses one `data:` line. `None` for keep-alives and comments.
pub fn parse_sse_line(line: &str) -> Option<Chunk> {
    let data = line.strip_prefix("data:")?.trim();
    if data.is_empty() {
        return None;
    }
    if data == "[DONE]" {
        return Some(Chunk::Done);
    }
    let parsed: StreamResponse = serde_json::from_str(data).ok()?;
    let choice = parsed.choices.into_iter().next()?;

    for tc in choice.delta.tool_calls {
        if let Some(f) = tc.function
            && let Some(name) = f.name
        {
            return Some(Chunk::ToolCall {
                name,
                arguments: f.arguments.unwrap_or_default(),
            });
        }
    }
    // An empty content delta is a keep-alive, not a token. Forwarding it
    // would tick the token counter for a message with no content in it.
    match choice.delta.content {
        Some(c) if !c.is_empty() => Some(Chunk::Token(c)),
        _ => None,
    }
}

/// Parses a non-streamed response body.
pub fn parse_completion(text: &str) -> Result<Completion, String> {
    let parsed: BlockingResponse =
        serde_json::from_str(text).map_err(|e| format!("not an OpenAI response: {e}"))?;
    let Some(choice) = parsed.choices.into_iter().next() else {
        return Err("no choices in the response".into());
    };
    Ok(Completion {
        content: choice.message.content.unwrap_or_default(),
        tool_calls: choice
            .message
            .tool_calls
            .into_iter()
            .filter_map(|t| t.function)
            .map(|f| (f.name, f.arguments))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_content_delta_becomes_a_token() {
        let line = r#"data: {"choices":[{"delta":{"content":"It walks"}}]}"#;
        assert_eq!(parse_sse_line(line), Some(Chunk::Token("It walks".into())));
    }

    #[test]
    fn the_terminator_is_recognised() {
        assert_eq!(parse_sse_line("data: [DONE]"), Some(Chunk::Done));
    }

    #[test]
    fn keep_alives_and_comments_are_not_tokens() {
        // An empty content delta arriving as a token would tick the rate
        // sparkline for a message carrying nothing.
        assert_eq!(parse_sse_line(": ping"), None);
        assert_eq!(parse_sse_line("data:"), None);
        assert_eq!(parse_sse_line(""), None);
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"content":""}}]}"#),
            None
        );
        assert_eq!(parse_sse_line(r#"data: {"choices":[]}"#), None);
    }

    #[test]
    fn a_malformed_line_is_skipped_rather_than_killing_the_stream() {
        // Servers do emit partial or unexpected frames. Dropping one is a
        // missing word; propagating it ends the answer mid-sentence.
        assert_eq!(parse_sse_line("data: {not json"), None);
    }

    #[test]
    fn a_tool_call_delta_is_carried_separately_from_the_text() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"function":{"name":"read_file","arguments":"{\"path\":\"a.py\"}"}}]}}]}"#;
        assert_eq!(
            parse_sse_line(line),
            Some(Chunk::ToolCall {
                name: "read_file".into(),
                arguments: r#"{"path":"a.py"}"#.into()
            })
        );
    }

    #[test]
    fn a_real_tool_call_and_json_in_the_prose_are_not_the_same_thing() {
        // The measurement A.7 asks for. A model that writes
        // `{"name": "read_file"}` into its answer has *failed* to call a
        // tool — that is mistral-nemo's dominant failure — and a check that
        // searched the body for a brace would score it as a success.
        let real = parse_completion(
            r#"{"choices":[{"message":{"content":"","tool_calls":[{"function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
        )
        .unwrap();
        assert!(real.used_tool_channel());
        assert_eq!(real.tool_calls[0].0, "read_file");

        let prose = parse_completion(
            r#"{"choices":[{"message":{"content":"I would call {\"name\": \"read_file\"} here."}}]}"#,
        )
        .unwrap();
        assert!(
            !prose.used_tool_channel(),
            "JSON in the content block is the failure being measured"
        );
        assert!(prose.content.contains("read_file"));
    }

    #[test]
    fn a_response_that_is_not_openai_shaped_is_an_error() {
        assert!(parse_completion("{}").is_err());
        assert!(parse_completion("nonsense").is_err());
    }

    #[test]
    fn the_default_endpoint_is_the_ollama_one_and_is_overridable() {
        // Built against the shape rather than the vendor: moving to
        // llama.cpp for grammar-constrained decoding is a URL change.
        let c = Config::default();
        assert!(c.base_url.contains("/v1"), "{}", c.base_url);
    }

    #[test]
    fn a_tool_serialises_into_the_shape_the_endpoint_expects() {
        let t = Tool::new(
            "read_file",
            "read a file",
            serde_json::json!({"type": "object", "properties": {}}),
        );
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], "read_file");
        assert!(v["function"]["parameters"].is_object());
    }
}
