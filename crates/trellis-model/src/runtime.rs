//! Talking to a serving runtime, and putting the schema in the field it reads.
//!
//! **Tier 2 ships a JSON Schema, not a compiled grammar.** The runtimes each
//! take a schema under a different key — llama.cpp `json_schema`, vLLM
//! `guided_json`, SGLang `json_schema` inside `sampling_params` — and
//! llama.cpp's `grammar` key wants GBNF rather than llguidance's Lark. JSON
//! Schema is the one format all of them accept, so it is the wire format, and
//! llguidance's job here is to have already validated that the schema compiles
//! (see `grammar.rs`).
//!
//! Getting the key wrong is the failure that matters: every runtime ignores
//! parameters it does not recognise, so a misnamed field produces *unconstrained*
//! generation that looks exactly like success until the model emits malformed
//! JSON under load. That is why each dialect is a separate variant with its own
//! test rather than one request shape with optional fields.
//!
//! **No TLS, deliberately.** Tier 2 targets a runtime on localhost, so this
//! speaks plain HTTP/1.1 over `tokio::net::TcpStream` and adds no dependency.
//! An `https://` base URL is refused with a message saying so rather than
//! silently downgraded — a remote endpoint is tier 1 or tier 4 and needs a
//! real client.

use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("{0}")]
    Config(String),
    #[error("cannot reach the runtime at {base}: {source}")]
    Connect {
        base: String,
        source: std::io::Error,
    },
    #[error("runtime returned HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("runtime response was not the expected shape: {0}")]
    Shape(String),
    #[error("runtime did not respond within {0:?}")]
    Timeout(Duration),
}

/// Which serving runtime is on the other end.
///
/// Probed rather than assumed (S18 asks the same of batching capability). A
/// wrong guess here does not error; it silently stops constraining output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// llama.cpp `/completion`.
    LlamaCpp,
    /// vLLM's OpenAI-compatible `/v1/completions`.
    VllmOpenAi,
    /// SGLang `/generate`.
    Sglang,
}

impl Dialect {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "llamacpp" | "llama.cpp" | "llama_cpp" => Some(Dialect::LlamaCpp),
            "vllm" | "openai" => Some(Dialect::VllmOpenAi),
            "sglang" => Some(Dialect::Sglang),
            _ => None,
        }
    }

    fn path(self) -> &'static str {
        match self {
            Dialect::LlamaCpp => "/completion",
            Dialect::VllmOpenAi => "/v1/completions",
            Dialect::Sglang => "/generate",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    /// Reported by the runtime where it offers a count. `0` means "not
    /// reported", never "free" — the budget's wall-clock and iteration
    /// ceilings apply regardless (S8).
    pub tokens: u64,
}

#[derive(Debug, Clone)]
pub struct RuntimeClient {
    host: String,
    port: u16,
    dialect: Dialect,
    model: Option<String>,
    timeout: Duration,
}

impl RuntimeClient {
    /// `base` is `http://host:port`.
    pub fn new(base: &str, dialect: Dialect, model: Option<String>) -> Result<Self, RuntimeError> {
        if base.starts_with("https://") {
            return Err(RuntimeError::Config(format!(
                "{base} is https. Tier 2 speaks plain HTTP to a local runtime and will \
                 not silently downgrade a TLS endpoint. A remote API is tier 1 or tier 4."
            )));
        }
        let rest = base
            .strip_prefix("http://")
            .ok_or_else(|| RuntimeError::Config(format!("{base} must start with http://")))?;
        let rest = rest.trim_end_matches('/');
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse().map_err(|_| {
                    RuntimeError::Config(format!("{base}: `{p}` is not a port number"))
                })?,
            ),
            None => (rest.to_string(), 80),
        };
        if host.is_empty() {
            return Err(RuntimeError::Config(format!("{base} has no host")));
        }
        Ok(Self {
            host,
            port,
            dialect,
            model,
            timeout: Duration::from_secs(300),
        })
    }

    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Builds the request body, putting `schema` in the key this dialect reads.
    ///
    /// `schema: None` is tier 4 — unconstrained generation that must be
    /// validated afterwards.
    pub fn body(&self, prompt: &str, schema: Option<&Value>, max_tokens: u32) -> Value {
        match self.dialect {
            Dialect::LlamaCpp => {
                let mut b = json!({
                    "prompt": prompt,
                    "n_predict": max_tokens,
                    "temperature": 0.0,
                    "cache_prompt": true,
                });
                if let Some(s) = schema {
                    b["json_schema"] = s.clone();
                }
                b
            }
            Dialect::VllmOpenAi => {
                let mut b = json!({
                    "model": self.model.clone().unwrap_or_else(|| "default".into()),
                    "prompt": prompt,
                    "max_tokens": max_tokens,
                    "temperature": 0.0,
                });
                if let Some(s) = schema {
                    b["guided_json"] = s.clone();
                }
                b
            }
            Dialect::Sglang => {
                let mut params = json!({
                    "max_new_tokens": max_tokens,
                    "temperature": 0.0,
                });
                if let Some(s) = schema {
                    params["json_schema"] = json!(s.to_string());
                }
                json!({ "text": prompt, "sampling_params": params })
            }
        }
    }

    /// Pulls the generated text out of whatever shape came back.
    pub fn extract(&self, v: &Value) -> Result<Completion, RuntimeError> {
        let shape = |what: &str| RuntimeError::Shape(format!("{what}; got {v}"));
        match self.dialect {
            Dialect::LlamaCpp => Ok(Completion {
                text: v["content"]
                    .as_str()
                    .ok_or_else(|| shape("expected a string `content`"))?
                    .to_string(),
                tokens: v["tokens_predicted"].as_u64().unwrap_or(0),
            }),
            Dialect::VllmOpenAi => Ok(Completion {
                text: v["choices"][0]["text"]
                    .as_str()
                    .ok_or_else(|| shape("expected `choices[0].text`"))?
                    .to_string(),
                tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            }),
            Dialect::Sglang => Ok(Completion {
                // SGLang returns either an object or a single-element array.
                text: v["text"]
                    .as_str()
                    .or_else(|| v[0]["text"].as_str())
                    .ok_or_else(|| shape("expected a string `text`"))?
                    .to_string(),
                tokens: v["meta_info"]["completion_tokens"].as_u64().unwrap_or(0),
            }),
        }
    }

    #[tracing::instrument(skip_all, fields(dialect = ?self.dialect, constrained = schema.is_some()))]
    pub async fn complete(
        &self,
        prompt: &str,
        schema: Option<&Value>,
        max_tokens: u32,
    ) -> Result<Completion, RuntimeError> {
        let body = self.body(prompt, schema, max_tokens).to_string();
        let raw = tokio::time::timeout(self.timeout, self.post(self.dialect.path(), &body))
            .await
            .map_err(|_| RuntimeError::Timeout(self.timeout))??;
        let v: Value = serde_json::from_str(&raw)
            .map_err(|e| RuntimeError::Shape(format!("response is not JSON: {e}; body: {raw}")))?;
        self.extract(&v)
    }

    /// Minimal HTTP/1.1 POST.
    ///
    /// `Connection: close` so the body is everything up to EOF — which avoids
    /// implementing chunked transfer decoding for a localhost call.
    async fn post(&self, path: &str, body: &str) -> Result<String, RuntimeError> {
        let addr = format!("{}:{}", self.host, self.port);
        let mut stream =
            TcpStream::connect(&addr)
                .await
                .map_err(|source| RuntimeError::Connect {
                    base: addr.clone(),
                    source,
                })?;

        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.host,
            body.len()
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|source| RuntimeError::Connect {
                base: addr.clone(),
                source,
            })?;

        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .await
            .map_err(|source| RuntimeError::Connect { base: addr, source })?;
        let text = String::from_utf8_lossy(&raw).into_owned();

        let (head, body) = text
            .split_once("\r\n\r\n")
            .ok_or_else(|| RuntimeError::Shape("no HTTP header terminator".into()))?;
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| RuntimeError::Shape(format!("no HTTP status in `{head}`")))?;
        if !(200..300).contains(&status) {
            return Err(RuntimeError::Http {
                status,
                body: body.chars().take(400).collect(),
            });
        }
        Ok(body.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(d: Dialect) -> RuntimeClient {
        RuntimeClient::new("http://127.0.0.1:8080", d, Some("m".into())).unwrap()
    }

    #[test]
    fn each_dialect_puts_the_schema_in_its_own_key() {
        // The failure this guards: runtimes ignore unknown parameters, so a
        // misnamed key yields unconstrained generation that looks like success
        // until the model emits malformed JSON.
        let schema = json!({"type": "object"});

        let b = client(Dialect::LlamaCpp).body("p", Some(&schema), 64);
        assert_eq!(b["json_schema"], schema);
        assert!(b.get("guided_json").is_none());

        let b = client(Dialect::VllmOpenAi).body("p", Some(&schema), 64);
        assert_eq!(b["guided_json"], schema);
        assert!(b.get("json_schema").is_none());

        let b = client(Dialect::Sglang).body("p", Some(&schema), 64);
        assert_eq!(
            b["sampling_params"]["json_schema"],
            json!(schema.to_string())
        );
    }

    #[test]
    fn tier_four_sends_no_schema_at_all() {
        for d in [Dialect::LlamaCpp, Dialect::VllmOpenAi, Dialect::Sglang] {
            let b = client(d).body("p", None, 64);
            let s = b.to_string();
            assert!(
                !s.contains("json_schema") && !s.contains("guided_json"),
                "{d:?} leaked a schema key when unconstrained: {s}"
            );
        }
    }

    #[test]
    fn each_dialect_reads_its_own_response_shape() {
        let c = client(Dialect::LlamaCpp);
        let got = c
            .extract(&json!({"content": "hi", "tokens_predicted": 7}))
            .unwrap();
        assert_eq!((got.text.as_str(), got.tokens), ("hi", 7));

        let c = client(Dialect::VllmOpenAi);
        let got = c
            .extract(&json!({"choices":[{"text":"hi"}], "usage":{"completion_tokens":3}}))
            .unwrap();
        assert_eq!((got.text.as_str(), got.tokens), ("hi", 3));

        let c = client(Dialect::Sglang);
        assert_eq!(c.extract(&json!({"text":"hi"})).unwrap().text, "hi");
    }

    #[test]
    fn an_unexpected_response_shape_is_an_error_not_an_empty_string() {
        // An empty completion would be reported to the loop as a malformed
        // tool call and retried, hiding a protocol mismatch behind retries.
        let e = client(Dialect::LlamaCpp)
            .extract(&json!({"unexpected": true}))
            .unwrap_err();
        assert!(matches!(e, RuntimeError::Shape(_)), "{e}");
    }

    #[test]
    fn missing_token_counts_report_zero_rather_than_failing() {
        let got = client(Dialect::LlamaCpp)
            .extract(&json!({"content": "hi"}))
            .unwrap();
        assert_eq!(got.tokens, 0);
    }

    #[test]
    fn a_tls_endpoint_is_refused_rather_than_downgraded() {
        let e =
            RuntimeClient::new("https://api.example.com", Dialect::VllmOpenAi, None).unwrap_err();
        assert!(
            format!("{e}").contains("will not silently downgrade"),
            "{e}"
        );
    }

    #[test]
    fn base_urls_are_parsed_or_rejected() {
        assert!(RuntimeClient::new("http://localhost:1234", Dialect::LlamaCpp, None).is_ok());
        assert!(RuntimeClient::new("localhost:1234", Dialect::LlamaCpp, None).is_err());
        assert!(RuntimeClient::new("http://localhost:notaport", Dialect::LlamaCpp, None).is_err());
    }

    /// A stub runtime, so the request shape and response parsing are tested
    /// without llama.cpp being installed.
    async fn stub(
        response: &'static str,
        status: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let received = String::from_utf8_lossy(&buf[..n]).into_owned();
            let out = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{response}",
                response.len()
            );
            sock.write_all(out.as_bytes()).await.unwrap();
            sock.shutdown().await.ok();
            received
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn a_round_trip_sends_the_schema_and_reads_the_completion() {
        let (base, server) = stub(
            r#"{"content":"{\"tool\":\"read_file\"}","tokens_predicted":11}"#,
            "200 OK",
        )
        .await;
        let c = RuntimeClient::new(&base, Dialect::LlamaCpp, None).unwrap();
        let schema = json!({"type":"object","properties":{"tool":{"type":"string"}}});

        let got = c.complete("hello", Some(&schema), 32).await.unwrap();
        assert_eq!(got.tokens, 11);
        assert!(got.text.contains("read_file"));

        let sent = server.await.unwrap();
        assert!(sent.starts_with("POST /completion HTTP/1.1"), "{sent}");
        assert!(
            sent.contains("json_schema"),
            "the schema must reach the runtime: {sent}"
        );
    }

    #[tokio::test]
    async fn a_non_2xx_response_is_an_error_carrying_the_body() {
        let (base, server) =
            stub(r#"{"error":"model not loaded"}"#, "503 Service Unavailable").await;
        let c = RuntimeClient::new(&base, Dialect::LlamaCpp, None).unwrap();
        let e = c.complete("hello", None, 32).await.unwrap_err();
        match e {
            RuntimeError::Http { status, body } => {
                assert_eq!(status, 503);
                assert!(body.contains("model not loaded"), "{body}");
            }
            other => panic!("expected an HTTP error, got {other}"),
        }
        server.await.ok();
    }

    #[tokio::test]
    async fn an_unreachable_runtime_says_so() {
        // Port 1 on loopback: reserved and never listening.
        let c = RuntimeClient::new("http://127.0.0.1:1", Dialect::LlamaCpp, None).unwrap();
        let e = c.complete("hello", None, 8).await.unwrap_err();
        assert!(matches!(e, RuntimeError::Connect { .. }), "{e}");
    }
}
