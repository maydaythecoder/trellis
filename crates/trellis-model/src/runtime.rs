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
    /// The provider refused the key. Carries the variable's name and never
    /// the provider's body, which may echo a masked form of the key.
    #[error(
        "the provider refused the key from {source_var} (HTTP {status}). Check that \
         {source_var} holds a valid key for this endpoint."
    )]
    Auth { status: u16, source_var: String },
    /// The request exceeded the provider's context window (ADR 0024 §5).
    #[error("the request exceeded the provider's context window: {0}")]
    ContextOverflow(String),
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

    /// The canonical name, so `Display` for a [`crate::ModelSpec`]
    /// round-trips. Crate-private: only that impl needs it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Dialect::LlamaCpp => "llamacpp",
            Dialect::VllmOpenAi => "vllm",
            Dialect::Sglang => "sglang",
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
        // The hosted transport's parser, so `[::1]` works here too.
        let (host, port) = crate::transport::split_authority(base, rest, 80)?;
        Ok(Self {
            host,
            port,
            dialect,
            model,
            timeout: Duration::from_secs(300),
        })
    }

    /// Can a TCP connection be opened at all?
    ///
    /// No HTTP, no generation, no dialect — just the connect. Cheap enough
    /// to run before a run starts, and that is the point: `cmd_run` takes a
    /// git snapshot before its first model call, so a refused connection
    /// discovered *inside* the loop costs a snapshot, a rollback and a trail
    /// file for a run that never made a call. Four of those in a row is what
    /// a dead runtime looked like.
    ///
    /// Five seconds, and the number is chosen so a *refusal* wins the race.
    /// A closed port on Windows takes a beat to answer, and
    /// `RuntimeError::Connect` — "the target machine actively refused it" —
    /// tells a user what to do, where a timeout only tells them to wait.
    /// This only ever targets a local runtime; a remote endpoint is tier 1
    /// or tier 4 and does not come through here.
    pub async fn reachable(&self) -> Result<(), RuntimeError> {
        const WINDOW: Duration = Duration::from_secs(5);
        let addr = crate::transport::socket_addr(&self.host, self.port);
        match tokio::time::timeout(WINDOW, TcpStream::connect(&addr)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(source)) => Err(RuntimeError::Connect { base: addr, source }),
            Err(_) => Err(RuntimeError::Timeout(WINDOW)),
        }
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
        let v: Value = serde_json::from_str(&raw).map_err(|e| {
            RuntimeError::Shape(format!("response is not JSON: {e}; body: {}", head(&raw)))
        })?;
        self.extract(&v)
    }

    /// Whether the runtime appears to honour guided decoding.
    ///
    /// Sends a schema demanding a key a model would not volunteer, and checks
    /// whether the reply carries it. **This cannot be certain**: a capable
    /// model might produce conforming JSON unprompted, so a positive result is
    /// "appears to honour" rather than proof.
    ///
    /// A *negative* result is solid — the runtime ignored the parameter — and
    /// that is the direction that matters, because the silent failure is
    /// believing output is constrained when it is not.
    pub async fn probe_constraint(&self) -> Result<bool, RuntimeError> {
        let schema = json!({
            "type": "object",
            // Without this the probe measures whitespace patience rather than
            // schema support: JSON allows unlimited spaces, and a weak model
            // will fill the token budget with them before reaching a value.
            // See `ToolCallWire::schema`.
            "x-guidance": { "whitespace_flexible": false },
            "properties": { "trellis_probe_ok": { "type": "boolean" } },
            "required": ["trellis_probe_ok"],
            "additionalProperties": false
        });
        let c = self
            .complete("Reply with JSON only.", Some(&schema), 32)
            .await?;
        let Some(start) = c.text.find('{') else {
            return Ok(false);
        };
        let Ok(v) = serde_json::from_str::<Value>(&c.text[start..]) else {
            return Ok(false);
        };
        Ok(v.get("trellis_probe_ok").is_some())
    }

    /// How many requests the runtime serves concurrently, when it says.
    ///
    /// vLLM and SGLang batch continuously by construction, so `None` means
    /// "their scheduler decides" rather than "unknown" — inventing a number
    /// there would be worse than declining to. llama.cpp reports
    /// `total_slots` from `/props`, which is 1 unless it was started with
    /// `--parallel`.
    ///
    /// S18: a user running four agents against a single-slot runtime gets no
    /// speedup and blames TRELLIS, so this is measured rather than assumed.
    pub async fn probe_slots(&self) -> Result<Option<u32>, RuntimeError> {
        match self.dialect {
            Dialect::VllmOpenAi | Dialect::Sglang => Ok(None),
            Dialect::LlamaCpp => {
                let raw = tokio::time::timeout(self.timeout, self.get("/props"))
                    .await
                    .map_err(|_| RuntimeError::Timeout(self.timeout))??;
                let v: Value = serde_json::from_str(&raw).map_err(|e| {
                    RuntimeError::Shape(format!("/props is not JSON: {e}; body: {}", head(&raw)))
                })?;
                Ok(v["total_slots"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok()))
            }
        }
    }

    async fn get(&self, path: &str) -> Result<String, RuntimeError> {
        self.request(&format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.host_header()
        ))
        .await
    }

    /// Minimal HTTP/1.1 POST.
    async fn post(&self, path: &str, body: &str) -> Result<String, RuntimeError> {
        self.request(&format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.host_header(),
            body.len()
        ))
        .await
    }

    fn host_header(&self) -> String {
        crate::transport::host_header(&self.host, self.port, 80)
    }

    /// Sends one request and reads the whole response.
    ///
    /// `Connection: close` means the body runs to EOF, which avoids
    /// implementing chunked transfer decoding for a localhost call.
    async fn request(&self, req: &str) -> Result<String, RuntimeError> {
        let addr = crate::transport::socket_addr(&self.host, self.port);
        let mut stream =
            TcpStream::connect(&addr)
                .await
                .map_err(|source| RuntimeError::Connect {
                    base: addr.clone(),
                    source,
                })?;

        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|source| RuntimeError::Connect {
                base: addr.clone(),
                source,
            })?;

        // Bounded as a hosted reply is (`transport::MAX_RESPONSE`): one byte
        // past the cap is read, so an over-long reply is told from one that
        // is exactly at it.
        let cap = crate::transport::MAX_RESPONSE;
        let mut raw = Vec::new();
        (&mut stream)
            .take(cap as u64 + 1)
            .read_to_end(&mut raw)
            .await
            .map_err(|source| RuntimeError::Connect { base: addr, source })?;
        if raw.len() > cap {
            return Err(RuntimeError::Shape(format!(
                "the runtime's response is larger than {} MiB, which no model reply is; \
                 nothing more was read",
                cap >> 20
            )));
        }
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

/// The first 400 characters of a body an error quotes, as the HTTP error
/// quotes it: the whole of a reply up to the size cap is not a message
/// anyone reads.
fn head(raw: &str) -> String {
    raw.chars().take(400).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a stub waits for the client to connect before giving up.
    ///
    /// Loopback, so a real connection arrives in microseconds. This only ever
    /// fires when the client did not make a request at all.
    const ACCEPT_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

    /// What a stub yields when nobody connected. No assertion matches it, so
    /// the test fails with a readable message instead of hanging.
    const NO_REQUEST: &str = "<no request was made>";

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
        // Bracketed IPv6, with and without a port: the shared parser.
        let c = RuntimeClient::new("http://[::1]", Dialect::LlamaCpp, None).unwrap();
        assert_eq!(
            (c.host.as_str(), c.port, c.host_header()),
            ("::1", 80, "[::1]".into())
        );
        let c = RuntimeClient::new("http://[::1]:8080/", Dialect::LlamaCpp, None).unwrap();
        assert_eq!(c.host_header(), "[::1]:8080");
    }

    /// A stub runtime, so the request shape and response parsing are tested
    /// without llama.cpp being installed.
    ///
    /// The accept is bounded, and that is not belt-and-braces. A mutant that
    /// makes the client skip its request — `get -> Ok(String::new())`, say —
    /// leaves nothing to accept, and an unbounded `accept().await` then hangs
    /// the test rather than failing it. Five of `trellis-model`'s mutants
    /// came back as timeouts for exactly that reason: no verdict, where the
    /// honest answer was "caught". A hang is also the worst failure mode in
    /// CI, because it stalls instead of reporting.
    ///
    /// On timeout the handle yields `NO_REQUEST`, which no assertion here
    /// matches, so the test fails in a second with a readable message.
    async fn stub(
        response: &'static str,
        status: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let accepted = tokio::time::timeout(ACCEPT_WINDOW, listener.accept()).await;
            let Ok(Ok((mut sock, _))) = accepted else {
                return NO_REQUEST.to_string();
            };
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
    async fn a_conforming_reply_reads_as_constrained() {
        let (base, server) = stub(
            r#"{"content":"{\"trellis_probe_ok\":true}","tokens_predicted":5}"#,
            "200 OK",
        )
        .await;
        let c = RuntimeClient::new(&base, Dialect::LlamaCpp, None).unwrap();
        assert!(c.probe_constraint().await.unwrap());
        server.await.ok();
    }

    #[tokio::test]
    async fn prose_instead_of_the_demanded_key_reads_as_unconstrained() {
        // The direction that matters. Believing output is constrained when it
        // is not is the silent failure; the reverse only costs a warning.
        let (base, server) = stub(
            r#"{"content":"Sure! Here is some JSON: {\"answer\": 42}","tokens_predicted":9}"#,
            "200 OK",
        )
        .await;
        let c = RuntimeClient::new(&base, Dialect::LlamaCpp, None).unwrap();
        assert!(!c.probe_constraint().await.unwrap());
        server.await.ok();
    }

    #[tokio::test]
    async fn llama_cpp_slot_count_comes_from_props() {
        let (base, server) = stub(r#"{"total_slots":4}"#, "200 OK").await;
        let c = RuntimeClient::new(&base, Dialect::LlamaCpp, None).unwrap();
        assert_eq!(c.probe_slots().await.unwrap(), Some(4));
        let sent = server.await.unwrap();
        assert!(sent.starts_with("GET /props"), "{sent}");
        assert!(
            sent.contains("Connection: close"),
            "the reader runs the body to EOF rather than parsing a length, \
             so the request has to ask for the connection to close: {sent}"
        );
    }

    #[tokio::test]
    async fn batching_runtimes_report_no_fixed_slot_count() {
        // vLLM and SGLang batch continuously; inventing a number would be
        // worse than saying the scheduler decides.
        for d in [Dialect::VllmOpenAi, Dialect::Sglang] {
            let c = RuntimeClient::new("http://127.0.0.1:1", d, None).unwrap();
            assert_eq!(c.probe_slots().await.unwrap(), None);
        }
    }

    #[tokio::test]
    async fn an_unreachable_runtime_says_so() {
        // Port 1 on loopback: reserved and never listening.
        let c = RuntimeClient::new("http://127.0.0.1:1", Dialect::LlamaCpp, None).unwrap();
        let e = c.complete("hello", None, 8).await.unwrap_err();
        assert!(matches!(e, RuntimeError::Connect { .. }), "{e}");
    }

    #[tokio::test]
    async fn a_runtime_that_does_not_report_slots_is_unknown_rather_than_one() {
        // S18, and the direction of the error matters. `auto_agents` maps
        // Unknown to 1, so being unable to read a slot count already produces
        // the conservative answer. What must not happen is this returning
        // `Some(1)`, which would be a *measurement* of single-stream serving
        // rather than an admission of not knowing — and a measurement is what
        // `supports_multi_agent` is entitled to believe.
        let (base, server) = stub(r#"{"model_path":"x.gguf"}"#, "200 OK").await;
        let c = RuntimeClient::new(&base, Dialect::LlamaCpp, None).unwrap();
        assert_eq!(
            c.probe_slots().await.unwrap(),
            None,
            "a missing total_slots is not knowledge that there is one slot"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_missing_props_endpoint_is_an_error_not_a_guess() {
        // An older llama.cpp without `/props` answers 404. Swallowing that
        // into `Ok(None)` would be defensible; making it an error is better,
        // because `doctor` can then say the probe failed rather than report
        // an unknown it did not actually establish — and the two are
        // different claims.
        //
        // The status survives as a status rather than being flattened into a
        // parse failure, so the message names 404 instead of complaining that
        // HTML is not JSON.
        let (base, server) = stub("<html>Not Found</html>", "404 Not Found").await;
        let c = RuntimeClient::new(&base, Dialect::LlamaCpp, None).unwrap();
        let e = c.probe_slots().await.unwrap_err();
        assert!(
            matches!(e, RuntimeError::Http { status: 404, .. }),
            "a 404 must stay a 404: {e}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_props_body_that_is_not_json_says_what_it_was_reading() {
        // Reached when something answers 200 that is not llama.cpp — a proxy,
        // or the wrong port. The error names `/props` so the user can tell
        // which probe failed rather than seeing a bare parse error.
        let (base, server) = stub("not json at all", "200 OK").await;
        let c = RuntimeClient::new(&base, Dialect::LlamaCpp, None).unwrap();
        let e = c.probe_slots().await.unwrap_err();
        assert!(matches!(e, RuntimeError::Shape(_)), "{e}");
        assert!(
            format!("{e}").contains("/props is not JSON"),
            "the error must name what it was reading: {e}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn the_configured_timeout_is_the_one_that_applies() {
        // `with_timeout` is how `doctor` keeps a probe from hanging a
        // terminal on a runtime that accepts a connection and then stalls.
        // The stub below accepts and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _held = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            // Held open, deliberately silent, until the test ends.
            std::future::pending::<()>().await;
            drop(sock);
        });

        let c = RuntimeClient::new(&format!("http://{addr}"), Dialect::LlamaCpp, None)
            .unwrap()
            .with_timeout(Duration::from_millis(150));
        let e = c.probe_slots().await.unwrap_err();
        assert!(
            matches!(e, RuntimeError::Timeout(d) if d == Duration::from_millis(150)),
            "the error must carry the timeout that fired, not a default: {e}"
        );
    }

    #[tokio::test]
    async fn a_runtime_reply_past_the_cap_is_refused_rather_than_buffered() {
        // The hosted transport's cap (review round 8, finding 8) holds for
        // the local client too, which read to end of stream unbounded.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
            let chunk = vec![b'x'; 1 << 20];
            for _ in 0..40 {
                if sock.write_all(&chunk).await.is_err() {
                    return;
                }
            }
        });
        let c = RuntimeClient::new(&format!("http://{addr}"), Dialect::LlamaCpp, None).unwrap();
        let e = c.probe_slots().await.unwrap_err();
        assert!(e.to_string().contains("32 MiB"), "{e}");
    }

    #[tokio::test]
    async fn reachable_answers_without_generating_anything() {
        // Used before a run takes its git snapshot, so it has to be cheap and
        // it has to be right. A listening socket is reachable even though
        // nothing ever replies on it — that is the question being asked.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let c = RuntimeClient::new(&format!("http://{addr}"), Dialect::LlamaCpp, None).unwrap();
        assert!(c.reachable().await.is_ok());

        // Port 1 on loopback: reserved and never listening.
        let dead = RuntimeClient::new("http://127.0.0.1:1", Dialect::LlamaCpp, None).unwrap();
        let e = dead.reachable().await.unwrap_err();
        assert!(
            matches!(e, RuntimeError::Connect { .. }),
            "a closed port must read as refused rather than as a timeout, \
             because refused is the one a user can act on: {e}"
        );
    }
}
