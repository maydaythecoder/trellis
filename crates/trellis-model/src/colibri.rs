//! Reads a colibri server's `GET /v1/models` and `GET /health` (ADR 0047).
//!
//! colibri is identified, not assumed: for a loopback base only, the served
//! model's `owned_by: "colibri"` names it (ADR 0047 decision 3). That is the
//! server's own claim, so nothing here changes where a key is sent: the
//! caller hands in the key ADR 0024 §3 resolves, or none.
//!
//! Everything a server returns is untrusted. Both reads go through
//! [`transport::get`], which caps the response at `MAX_RESPONSE`, bounds the
//! whole request by [`READ_TIMEOUT`] and follows no redirect; the bodies are
//! parsed into exact types, and one that does not fit is an error.
//!
//! `/health` sits at the server root while an openai-chat base ends in
//! `/v1`, so both reads go to the base's origin, whatever its path.
//!
//! [`probe_tools`] sends the one tools probe of ADR 0047 decision 4 through
//! [`transport::post`] under the same cap, bound and origin rule.

use crate::runtime::RuntimeError;
use crate::secret::ApiKey;
use crate::transport::{self, Auth, Endpoint, SendError};
use serde::Deserialize;
use std::time::Duration;

/// The bound on each read, connect included. ADR 0047 decision 4 bounds
/// the tools probe at 10 s; a read of a loopback server's own state needs
/// no longer.
pub const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// What `GET /v1/models` said about the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// colibri, serving this model id.
    Identified { id: String },
    /// An OpenAI-compatible server that is not colibri; `owned_by` is what
    /// its model list said.
    NotColibri { owned_by: String },
    /// The server refused the key, or the lack of one (401 or 403). colibri
    /// started with `--api-key` answers a request without it this way, so
    /// this is not "not colibri".
    Unauthorized { status: u16 },
    /// No `/v1/models` route (404).
    Absent { status: u16 },
    /// The base is not this machine, so nothing was read: remote endpoints
    /// are never identified (ADR 0047 decision 3).
    NotLoopback,
}

/// What `GET /health` reported beyond liveness. colibri adds these only for
/// an authenticated request or a server with no key, and `tiers` and
/// `hwinfo` only once its engine has reported them.
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub kv_slots: Option<u32>,
    pub tiers: Option<Tiers>,
    pub hwinfo: Option<HwInfo>,
}

impl Health {
    /// True when the body held liveness only (`{"status": "ok"}`), as
    /// colibri answers a request without its key.
    pub fn liveness_only(&self) -> bool {
        self.kv_slots.is_none() && self.tiers.is_none() && self.hwinfo.is_none()
    }
}

/// Where colibri's routed experts live: counts per tier, and resident GB
/// for VRAM and RAM. colibri reports no GB figure for disk.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tiers {
    pub vram: u64,
    pub ram: u64,
    pub disk: u64,
    pub vram_gb: f64,
    pub ram_gb: f64,
}

/// The machine colibri runs on, as its engine reported it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HwInfo {
    pub cores: u32,
    pub ram_total_gb: f64,
    pub ram_avail_gb: f64,
    pub gpus: u32,
    pub vram_total_gb: f64,
    pub cpu: String,
    pub gpu: String,
}

/// `/v1/models`: the fields identification reads. Others are ignored.
#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelObject>,
}

#[derive(Deserialize)]
struct ModelObject {
    id: String,
    owned_by: String,
}

/// `/health`: `scheduler` and `continue_assistant` are not read.
#[derive(Deserialize)]
struct HealthBody {
    status: String,
    kv_slots: Option<u32>,
    tiers: Option<Tiers>,
    hwinfo: Option<HwInfo>,
}

/// Reads `GET /v1/models` at `base`'s origin and says whether the server is
/// colibri. A base that is not loopback returns [`Identity::NotLoopback`]
/// before any connection is opened.
pub async fn identify(base: &str, key: Option<&ApiKey>) -> Result<Identity, RuntimeError> {
    let Some(origin) = loopback_origin(base)? else {
        return Ok(Identity::NotLoopback);
    };
    let r = read(&origin, base, "/v1/models", key).await?;
    match r.status {
        200..=299 => {}
        401 | 403 => return Ok(Identity::Unauthorized { status: r.status }),
        404 => return Ok(Identity::Absent { status: r.status }),
        _ => return Err(transport::status_error(&r, key)),
    }
    let list: ModelList = parse(&r.body, "/v1/models")?;
    // colibri lists exactly the one id it serves (c/openai_server.py:4360-4362
    // at ce370e8), so the first entry decides.
    match list.data.into_iter().next() {
        Some(m) if m.owned_by == "colibri" => Ok(Identity::Identified { id: m.id }),
        Some(m) => Ok(Identity::NotColibri {
            owned_by: m.owned_by,
        }),
        None => Err(RuntimeError::Shape(
            "/v1/models listed no model".to_string(),
        )),
    }
}

/// Reads `GET /health` at `base`'s origin. A base that is not loopback is a
/// configuration error, as nothing remote is read.
pub async fn health(base: &str, key: Option<&ApiKey>) -> Result<Health, RuntimeError> {
    let Some(origin) = loopback_origin(base)? else {
        return Err(RuntimeError::Config(format!(
            "{base} is not this machine; only a loopback server's /health is read"
        )));
    };
    let r = read(&origin, base, "/health", key).await?;
    if !(200..300).contains(&r.status) {
        return Err(transport::status_error(&r, key));
    }
    let b: HealthBody = parse(&r.body, "/health")?;
    if b.status != "ok" {
        return Err(RuntimeError::Shape(format!(
            "/health status is `{}`, not `ok`",
            b.status.chars().take(40).collect::<String>()
        )));
    }
    Ok(Health {
        kv_slots: b.kv_slots,
        tiers: b.tiers,
        hwinfo: b.hwinfo,
    })
}

/// What colibri said to one chat request carrying one tool (ADR 0047
/// decision 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolsProbe {
    /// A 400 whose `param` is `tools`: the served family's template cannot
    /// render tools. `fallback_named` is true only when colibri's own
    /// message named `COLI_TOOL_FALLBACK` (OLMoE's and Qwen3.6's do,
    /// Inkling's does not).
    Refused { fallback_named: bool },
    /// A 200: the template accepted the tools. Not evidence the model calls
    /// them well.
    Accepted,
    /// Anything else. `reason` is TRELLIS's, written from the HTTP status or
    /// the transport error and never from the response body.
    NotVerified { reason: String },
}

/// colibri's error envelope (`error_object`, c/openai_server.py:75-77 at
/// ce370e8).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorBody {
    error: ErrorObject,
}

/// All four fields, so a body of another shape is not read as a refusal;
/// only `param` and `message` decide anything.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorObject {
    message: String,
    #[serde(rename = "type")]
    _type: String,
    param: Option<String>,
    #[serde(rename = "code")]
    _code: String,
}

/// Sends one `POST <origin>/v1/chat/completions` to the loopback colibri
/// at `base`, carrying one tool and `max_tokens: 1`, and says whether the
/// served family accepts tools. The path is the origin's whatever `base`'s
/// protocol: an `anthropic:` base has no `/v1` (ADR 0047 decision 2).
///
/// A base that is not loopback is a configuration error, returned before
/// any connection: remote endpoints are never probed. Every other outcome,
/// a transport failure included, is a [`ToolsProbe`].
pub async fn probe_tools(
    base: &str,
    served_id: &str,
    key: Option<&ApiKey>,
) -> Result<ToolsProbe, RuntimeError> {
    let Some(origin) = loopback_origin(base)? else {
        return Err(RuntimeError::Config(format!(
            "{base} is not this machine; only a loopback server is sent the tools probe"
        )));
    };
    let body = serde_json::json!({
        "model": served_id,
        "messages": [{"role": "user", "content": "Say ok."}],
        "tools": [{
            "type": "function",
            "function": {
                "name": "noop",
                "description": "Does nothing.",
                "parameters": {"type": "object", "properties": {}},
            },
        }],
        "max_tokens": 1,
    })
    .to_string();
    let auth = key.map(|k| (Auth::Bearer, k));
    let r = match transport::post(
        &origin,
        None,
        "/v1/chat/completions",
        &[],
        auth,
        &body,
        READ_TIMEOUT,
    )
    .await
    {
        Ok(r) => r,
        // `Deadline` and `Stream` come only from a streamed request, which
        // this is not; matched with their nearest kin, not left to a `_`.
        Err(SendError::Timeout | SendError::Deadline) => {
            return Ok(ToolsProbe::NotVerified {
                reason: format!("no answer within {} s", READ_TIMEOUT.as_secs()),
            });
        }
        // A `Broken` message can quote the server's status line, so it is
        // not repeated. The others are TRELLIS's own: an address and an OS
        // error, or a configuration fault, and never a response byte.
        Err(SendError::Broken(_) | SendError::Stream(_)) => {
            return Ok(ToolsProbe::NotVerified {
                reason: "the connection broke, or the reply was not well-formed HTTP".to_string(),
            });
        }
        Err(e @ (SendError::Connect(_) | SendError::ConnectTimedOut(_) | SendError::Config(_))) => {
            return Ok(ToolsProbe::NotVerified {
                reason: e.to_string(),
            });
        }
    };
    Ok(match r.status {
        200 => ToolsProbe::Accepted,
        400 => match serde_json::from_str::<ErrorBody>(&r.body) {
            Ok(b) if b.error.param.as_deref() == Some("tools") => ToolsProbe::Refused {
                fallback_named: b.error.message.contains("COLI_TOOL_FALLBACK"),
            },
            Ok(_) => ToolsProbe::NotVerified {
                reason: "HTTP 400 about a parameter other than tools".to_string(),
            },
            Err(_) => ToolsProbe::NotVerified {
                reason: "HTTP 400 with a body that is not colibri's error shape".to_string(),
            },
        },
        s => ToolsProbe::NotVerified {
            reason: format!("HTTP {s}"),
        },
    })
}

/// `base`'s origin as an endpoint with no path, or `None` when its host is
/// not loopback. The host is read before [`Endpoint::parse`], which refuses
/// plain http to a remote host: such a base is not this machine, which is
/// an answer, not an error.
fn loopback_origin(base: &str) -> Result<Option<Endpoint>, RuntimeError> {
    let (rest, default_port) = if let Some(r) = base.strip_prefix("https://") {
        (r, 443)
    } else if let Some(r) = base.strip_prefix("http://") {
        (r, 80)
    } else {
        return Err(RuntimeError::Config(format!(
            "{base} must start with http:// or https://"
        )));
    };
    let authority = rest.split('/').next().unwrap_or("");
    let (host, _) = transport::split_authority(base, authority, default_port)?;
    if !transport::is_loopback(&host) {
        return Ok(None);
    }
    let mut ep = Endpoint::parse(base)?;
    ep.prefix.clear();
    Ok(Some(ep))
}

async fn read(
    origin: &Endpoint,
    base: &str,
    path: &str,
    key: Option<&ApiKey>,
) -> Result<transport::Response, RuntimeError> {
    let auth = key.map(|k| (Auth::Bearer, k));
    transport::get(origin, path, auth, READ_TIMEOUT)
        .await
        .map_err(|e| match e {
            SendError::Connect(m)
            | SendError::ConnectTimedOut(m)
            | SendError::Broken(m)
            | SendError::Stream(m) => RuntimeError::Connect {
                base: base.to_string(),
                source: std::io::Error::other(m),
            },
            SendError::Timeout | SendError::Deadline => RuntimeError::Timeout(READ_TIMEOUT),
            SendError::Config(m) => RuntimeError::Config(m),
        })
}

/// A body into `T`, or a `Shape` error that quotes none of it: the error
/// is printed, and the body is the server's.
fn parse<'a, T: Deserialize<'a>>(body: &'a str, what: &str) -> Result<T, RuntimeError> {
    serde_json::from_str(body)
        .map_err(|e| RuntimeError::Shape(format!("{what} is not the expected shape: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Fixtures are transcribed from colibri's source at ce370e8
    // (c/openai_server.py, v1.12.1), not captured from a running server.
    // `send_json` writes compact JSON (`separators=(",", ":")`, :4151).

    /// ce370e8, c/openai_server.py:3885-3886 (`model_object`, `owned_by:
    /// "colibri"`) inside the list sent at :4360-4362.
    const MODELS_COLIBRI: &str = r#"{"object":"list","data":[{"id":"glm-5.2","object":"model","created":1758800000,"owned_by":"colibri"}]}"#;

    /// The same list shape with another owner, as vLLM answers. Shape from
    /// ce370e8, c/openai_server.py:3885-3886, with `owned_by` changed.
    const MODELS_VLLM: &str = r#"{"object":"list","data":[{"id":"Qwen/Qwen3-8B","object":"model","created":1758800000,"owned_by":"vllm"}]}"#;

    /// ce370e8, c/openai_server.py:4190-4193 (`require_auth`: 401
    /// `invalid_api_key`, `authentication_error`) in `error_object`'s
    /// shape (:75-77).
    const UNAUTHORIZED: &str = r#"{"error":{"message":"Invalid or missing API key.","type":"authentication_error","param":null,"code":"invalid_api_key"}}"#;

    /// ce370e8, c/openai_server.py:4366 (an unknown GET path: 404
    /// `not_found`) in `error_object`'s shape (:75-77).
    const NOT_FOUND: &str = r#"{"error":{"message":"Not found.","type":"invalid_request_error","param":null,"code":"not_found"}}"#;

    /// ce370e8, c/openai_server.py:4318-4331 (authed `/health`), with
    /// `scheduler` from :211-218, `tiers` from :3634-3636 and `hwinfo` from
    /// :3608-3614.
    const HEALTH_FULL: &str = r#"{"status":"ok","scheduler":{"active":0,"queued":0,"capacity":16,"max_queue":8,"queue_timeout_seconds":300,"admitted":3,"completed":3,"failed":0,"rejected":0,"timed_out":0,"cancelled":0},"kv_slots":16,"continue_assistant":true,"tiers":{"vram":120,"ram":2400,"disk":17000,"vram_gb":22.5,"ram_gb":96.0},"hwinfo":{"cores":32,"ram_total_gb":128.0,"ram_avail_gb":101.3,"gpus":1,"vram_total_gb":24.0,"cpu":"AMD Ryzen 9 7950X","gpu":"NVIDIA GeForce RTX 4090"}}"#;

    /// ce370e8, c/openai_server.py:4322 (public liveness, unauthenticated).
    const HEALTH_LIVENESS: &str = r#"{"status":"ok"}"#;

    /// A loopback stub answering each connection in turn with `(status,
    /// body)`, or with `head` alone when `body` is `None`, and returning the
    /// raw requests it received.
    async fn stub(
        replies: Vec<(&'static str, Option<&'static str>)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let h = tokio::spawn(async move {
            let mut got = Vec::new();
            for (status, body) in replies {
                let Ok(Ok((mut s, _))) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept()).await
                else {
                    break;
                };
                got.push(read_request(&mut s).await);
                let out = match body {
                    Some(b) => format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
                        b.len()
                    ),
                    None => format!("HTTP/1.1 {status}\r\n\r\n"),
                };
                let _ = s.write_all(out.as_bytes()).await;
                s.shutdown().await.ok();
            }
            got
        });
        (format!("http://{addr}"), h)
    }

    /// One whole request: the head, then as many body bytes as its
    /// `Content-Length` names, so a POST body sent in a later segment than
    /// its head is not cut off.
    async fn read_request(s: &mut tokio::net::TcpStream) -> String {
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let text = String::from_utf8_lossy(&raw).into_owned();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text[..end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if raw.len() >= end + 4 + len {
                    return text;
                }
            }
            let n = s.read(&mut buf).await.unwrap();
            if n == 0 {
                return String::from_utf8_lossy(&raw).into_owned();
            }
            raw.extend_from_slice(&buf[..n]);
        }
    }

    #[tokio::test]
    async fn colibri_identify_reads_owned_by_colibri_as_identified() {
        let (base, _h) = stub(vec![("200 OK", Some(MODELS_COLIBRI))]).await;
        assert_eq!(
            identify(&base, None).await.unwrap(),
            Identity::Identified {
                id: "glm-5.2".into()
            }
        );
    }

    #[tokio::test]
    async fn colibri_identify_reads_another_owner_as_not_colibri() {
        // The risk: any OpenAI-compatible server taken for colibri and sent
        // its request profile.
        let (base, _h) = stub(vec![("200 OK", Some(MODELS_VLLM))]).await;
        assert_eq!(
            identify(&base, None).await.unwrap(),
            Identity::NotColibri {
                owned_by: "vllm".into()
            }
        );
    }

    #[tokio::test]
    async fn colibri_identify_reads_colibri_s_401_as_unauthorized_not_not_colibri() {
        // The risk: colibri started with `--api-key`, asked without it, read
        // as some other server.
        let (base, _h) = stub(vec![
            ("401 Unauthorized", Some(UNAUTHORIZED)),
            ("403 Forbidden", Some(UNAUTHORIZED)),
        ])
        .await;
        for status in [401, 403] {
            assert_eq!(
                identify(&base, None).await.unwrap(),
                Identity::Unauthorized { status }
            );
        }
    }

    #[tokio::test]
    async fn colibri_identify_reads_a_404_as_absent() {
        let (base, _h) = stub(vec![("404 Not Found", Some(NOT_FOUND))]).await;
        assert_eq!(
            identify(&base, None).await.unwrap(),
            Identity::Absent { status: 404 }
        );
    }

    #[tokio::test]
    async fn colibri_health_reads_kv_slots_tiers_and_hwinfo_from_a_full_body() {
        let (base, _h) = stub(vec![("200 OK", Some(HEALTH_FULL))]).await;
        let h = health(&base, None).await.unwrap();
        assert!(!h.liveness_only());
        assert_eq!(h.kv_slots, Some(16));
        // Five fields and no disk GB: colibri reports none (:3634-3636).
        assert_eq!(
            h.tiers,
            Some(Tiers {
                vram: 120,
                ram: 2400,
                disk: 17000,
                vram_gb: 22.5,
                ram_gb: 96.0,
            })
        );
        assert_eq!(
            h.hwinfo,
            Some(HwInfo {
                cores: 32,
                ram_total_gb: 128.0,
                ram_avail_gb: 101.3,
                gpus: 1,
                vram_total_gb: 24.0,
                cpu: "AMD Ryzen 9 7950X".into(),
                gpu: "NVIDIA GeForce RTX 4090".into(),
            })
        );
    }

    #[tokio::test]
    async fn colibri_health_reads_a_liveness_only_body_as_liveness_only() {
        let (base, _h) = stub(vec![("200 OK", Some(HEALTH_LIVENESS))]).await;
        let h = health(&base, None).await.unwrap();
        assert!(h.liveness_only());
        assert_eq!((h.kv_slots, h.tiers, h.hwinfo), (None, None, None));
    }

    #[tokio::test]
    async fn colibri_health_refuses_a_field_of_the_wrong_type() {
        // The risk: a server's body taken loosely, so a string where a count
        // belongs reads as a count.
        let (base, _h) = stub(vec![("200 OK", Some(r#"{"status":"ok","kv_slots":"16"}"#))]).await;
        let e = health(&base, None).await.unwrap_err();
        assert!(matches!(e, RuntimeError::Shape(_)), "{e}");
    }

    /// The request line of each read the stub received from `base`.
    async fn request_lines(base: String) -> Vec<String> {
        let (stub_base, h) = stub(vec![
            ("200 OK", Some(MODELS_COLIBRI)),
            ("200 OK", Some(HEALTH_LIVENESS)),
        ])
        .await;
        let base = format!("{stub_base}{base}");
        identify(&base, None).await.unwrap();
        health(&base, None).await.unwrap();
        h.await
            .unwrap()
            .iter()
            .map(|r| r.lines().next().unwrap_or("").to_string())
            .collect()
    }

    #[tokio::test]
    async fn colibri_reads_go_to_the_origin_from_a_v1_base() {
        // The risk: `/health` read at `/v1/health`, which colibri does not
        // serve, and `/v1/models` at `/v1/v1/models`.
        assert_eq!(
            request_lines("/v1".into()).await,
            ["GET /v1/models HTTP/1.1", "GET /health HTTP/1.1"]
        );
    }

    #[tokio::test]
    async fn colibri_reads_go_to_the_origin_from_a_base_with_no_path() {
        assert_eq!(
            request_lines(String::new()).await,
            ["GET /v1/models HTTP/1.1", "GET /health HTTP/1.1"]
        );
    }

    #[tokio::test]
    async fn colibri_identify_refuses_a_body_past_the_cap() {
        // A declared length past the cap is refused on the header, as `post`
        // refuses it, before any of the gigabyte it promises is read.
        let (base, _h) = stub(vec![("200 OK\r\nContent-Length: 1073741824", None)]).await;
        let e = identify(&base, None).await.unwrap_err().to_string();
        assert!(e.contains("32 MiB"), "{e}");
    }

    #[tokio::test]
    async fn colibri_a_redirect_is_not_followed() {
        // The risk: a 302 followed to another host, carrying the key there.
        let (base, h) = stub(vec![(
            "302 Found\r\nLocation: http://10.0.0.5:8000/v1/models\r\nContent-Length: 0",
            None,
        )])
        .await;
        let key = ApiKey::for_test(&format!("colibri-{}", std::process::id()), "COLI_API_KEY");
        let e = identify(&base, Some(&key)).await.unwrap_err();
        assert!(matches!(e, RuntimeError::Http { status: 302, .. }), "{e}");
        assert_eq!(h.await.unwrap().len(), 1, "one request, to the stub");
    }

    #[tokio::test]
    async fn colibri_a_401_error_carries_the_status_and_never_the_body() {
        // The risk: a refusal body echoing the key it was sent, printed.
        let (base, _h) = stub(vec![("401 Unauthorized", Some(UNAUTHORIZED))]).await;
        let key = ApiKey::for_test(&format!("colibri-{}", std::process::id()), "COLI_API_KEY");
        let e = health(&base, Some(&key)).await.unwrap_err();
        assert!(
            matches!(&e, RuntimeError::Auth { status: 401, source_var } if source_var == "COLI_API_KEY"),
            "{e}"
        );
        assert!(!e.to_string().contains("Invalid or missing"), "{e}");
    }

    #[tokio::test]
    async fn colibri_identify_sends_a_given_key_as_bearer() {
        // Built at run time, so no key-shaped literal is in the source.
        let value = format!("colibri-test-{}", std::process::id());
        let key = ApiKey::for_test(&value, "COLI_API_KEY");
        let (base, h) = stub(vec![("200 OK", Some(MODELS_COLIBRI))]).await;
        identify(&base, Some(&key)).await.unwrap();
        let got = h.await.unwrap();
        assert!(
            got[0].contains(&format!("\r\nAuthorization: Bearer {value}\r\n")),
            "{}",
            got[0]
        );
    }

    #[tokio::test]
    async fn colibri_reads_send_no_key_header_without_a_key() {
        let (base, h) = stub(vec![
            ("200 OK", Some(MODELS_COLIBRI)),
            ("200 OK", Some(HEALTH_LIVENESS)),
        ])
        .await;
        identify(&base, None).await.unwrap();
        health(&base, None).await.unwrap();
        for raw in h.await.unwrap() {
            let lower = raw.to_ascii_lowercase();
            assert!(
                !lower.contains("authorization") && !lower.contains("x-api-key"),
                "{raw}"
            );
        }
    }

    #[tokio::test]
    async fn colibri_identify_skips_a_remote_base() {
        // No stub: a connection to either address would fail or hang, so an
        // `Ok(NotLoopback)` shows none was opened.
        for base in ["https://api.openai.com/v1", "http://10.0.0.5:8000/v1"] {
            assert_eq!(
                identify(base, None).await.unwrap(),
                Identity::NotLoopback,
                "{base}"
            );
        }
    }

    // The tools probe's fixtures. colibri's source is not in this
    // repository, so the `message` wording below is reconstructed from the
    // cited lines as ADR 0047 and docs/inventory/inventory-colibri.md
    // describe them, not copied: the envelope is `error_object`'s
    // (c/openai_server.py:75-77 at ce370e8), the code `unsupported_parameter`
    // and the param `tools`. The probe reads only `param` and whether the
    // message names `COLI_TOOL_FALLBACK`, never the rest of the wording.

    /// ce370e8, c/openai_server.py:1364-1367 (OLMoE refuses `tools` while
    /// rendering, naming the fallback setting); Qwen3.6's :1423-1426 is the
    /// same shape.
    const TOOLS_REFUSED_OLMOE: &str = r#"{"error":{"message":"OLMoE has no tool-calling chat template; set COLI_TOOL_FALLBACK=1 to render tools as text.","type":"invalid_request_error","param":"tools","code":"unsupported_parameter"}}"#;

    /// ce370e8, c/openai_server.py:1707-1708 (Inkling refuses `tools`, with
    /// no fallback named).
    const TOOLS_REFUSED_INKLING: &str = r#"{"error":{"message":"Inkling does not support tools.","type":"invalid_request_error","param":"tools","code":"unsupported_parameter"}}"#;

    /// A chat completion cut at one token: ce370e8, c/openai_server.py:
    /// 5169-5241 (the non-streaming reply), `finish_reason` `length`.
    const TOOLS_ACCEPTED: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","created":1758800000,"model":"glm-5.2","choices":[{"index":0,"message":{"role":"assistant","content":"I"},"finish_reason":"length"}],"usage":{"prompt_tokens":40,"completion_tokens":1,"total_tokens":41}}"#;

    /// Not colibri's: a hostile 500 whose body carries an ESC byte and a
    /// marker word, to show neither reaches the reason. Envelope from
    /// ce370e8, c/openai_server.py:75-77.
    const TOOLS_HOSTILE_500: &str = "{\"error\":{\"message\":\"\x1b[2Jcanary-body-word\",\"type\":\"server_error\",\"param\":null,\"code\":\"internal_error\"}}";

    #[tokio::test]
    async fn colibri_tools_an_olmoe_style_400_is_refused_with_the_fallback_named() {
        let (base, _h) = stub(vec![("400 Bad Request", Some(TOOLS_REFUSED_OLMOE))]).await;
        assert_eq!(
            probe_tools(&base, "olmoe", None).await.unwrap(),
            ToolsProbe::Refused {
                fallback_named: true
            }
        );
    }

    #[tokio::test]
    async fn colibri_tools_an_inkling_style_400_is_refused_with_no_fallback_named() {
        // The risk: telling an Inkling user to set a variable that cannot
        // help.
        let (base, _h) = stub(vec![("400 Bad Request", Some(TOOLS_REFUSED_INKLING))]).await;
        assert_eq!(
            probe_tools(&base, "inkling", None).await.unwrap(),
            ToolsProbe::Refused {
                fallback_named: false
            }
        );
    }

    #[tokio::test]
    async fn colibri_tools_a_200_is_accepted() {
        let (base, _h) = stub(vec![("200 OK", Some(TOOLS_ACCEPTED))]).await;
        assert_eq!(
            probe_tools(&base, "glm-5.2", None).await.unwrap(),
            ToolsProbe::Accepted
        );
    }

    #[tokio::test]
    async fn colibri_tools_a_500_is_not_verified_and_its_reason_quotes_no_body() {
        // The risk: a server's bytes, escape sequences included, printed to
        // the user's terminal as TRELLIS's own words.
        let (base, _h) = stub(vec![("500 Internal Server Error", Some(TOOLS_HOSTILE_500))]).await;
        let key = ApiKey::for_test(&format!("colibri-{}", std::process::id()), "COLI_API_KEY");
        let ToolsProbe::NotVerified { reason } =
            probe_tools(&base, "glm-5.2", Some(&key)).await.unwrap()
        else {
            panic!("a 500 must be NotVerified");
        };
        assert!(reason.contains("500"), "{reason}");
        assert!(!reason.contains('\x1b'), "{reason:?}");
        assert!(!reason.contains("canary-body-word"), "{reason}");
        assert!(!reason.contains("internal_error"), "{reason}");
        assert!(!reason.contains(key.expose()), "{reason}");
    }

    #[tokio::test]
    async fn colibri_tools_a_malformed_status_line_is_not_verified_and_not_quoted() {
        // The risk: the transport's parse error, which quotes the status
        // line, passed on as the reason with the server's escape bytes.
        let (base, _h) = stub(vec![("\x1b[2Jcanary-status-word", None)]).await;
        let ToolsProbe::NotVerified { reason } = probe_tools(&base, "glm-5.2", None).await.unwrap()
        else {
            panic!("a reply with no status must be NotVerified");
        };
        assert!(!reason.contains('\x1b'), "{reason:?}");
        assert!(!reason.contains("canary-status-word"), "{reason}");
    }

    #[tokio::test]
    async fn colibri_tools_a_400_about_another_param_is_not_verified() {
        // The risk: any 400 read as a tools refusal, stopping a run that
        // would have worked.
        let (base, _h) = stub(vec![("400 Bad Request", Some(NOT_FOUND))]).await;
        assert!(matches!(
            probe_tools(&base, "glm-5.2", None).await.unwrap(),
            ToolsProbe::NotVerified { .. }
        ));
    }

    /// The raw request the stub received when probed from `stub + path`.
    async fn probed_request(path: &str, key: Option<&ApiKey>) -> String {
        let (stub_base, h) = stub(vec![("200 OK", Some(TOOLS_ACCEPTED))]).await;
        probe_tools(&format!("{stub_base}{path}"), "glm-5.2", key)
            .await
            .unwrap();
        let got = h.await.unwrap();
        assert_eq!(got.len(), 1, "exactly one request");
        got.into_iter().next().unwrap()
    }

    #[tokio::test]
    async fn colibri_tools_the_probe_goes_to_v1_chat_completions_from_a_v1_base_and_a_bare_one() {
        // The risk: an `anthropic:` base (no `/v1`) probed at
        // `/chat/completions`, or an openai-chat base at
        // `/v1/v1/chat/completions`; colibri serves neither.
        for path in ["/v1", ""] {
            let raw = probed_request(path, None).await;
            assert_eq!(
                raw.lines().next().unwrap(),
                "POST /v1/chat/completions HTTP/1.1",
                "from base path `{path}`"
            );
        }
    }

    #[tokio::test]
    async fn colibri_tools_the_body_asks_for_one_token_with_one_tool() {
        // The risk: a probe that lets the model generate, and so outruns its
        // 10 s bound, or that asks nothing about tools.
        let raw = probed_request("/v1", None).await;
        let body = raw.split_once("\r\n\r\n").unwrap().1;
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["max_tokens"], 1, "{body}");
        assert_eq!(v["tools"].as_array().map(Vec::len), Some(1), "{body}");
        assert_eq!(v["model"], "glm-5.2", "{body}");
        let messages = v["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1, "{body}");
        assert_eq!(messages[0]["role"], "user", "{body}");
    }

    #[tokio::test]
    async fn colibri_tools_the_key_is_sent_only_when_given() {
        let lower = probed_request("", None).await.to_ascii_lowercase();
        assert!(
            !lower.contains("authorization") && !lower.contains("x-api-key"),
            "{lower}"
        );
        let value = format!("colibri-test-{}", std::process::id());
        let key = ApiKey::for_test(&value, "COLI_API_KEY");
        let raw = probed_request("", Some(&key)).await;
        assert!(
            raw.contains(&format!("\r\nAuthorization: Bearer {value}\r\n")),
            "{raw}"
        );
    }

    #[test]
    fn colibri_tools_a_remote_base_is_refused_before_any_connection() {
        // Polled once with no tokio runtime: a build that went on to send
        // would panic in the request's timeout, which needs a runtime, and
        // were that gone, in resolving the `.invalid` name on tokio's
        // blocking pool, both before any packet leaves. So even a broken
        // build cannot reach the network from this test. https, because
        // `Endpoint::parse` already refuses plain http to a remote host and
        // would hide a missing loopback check.
        let fut = probe_tools("https://colibri.invalid:8000/v1", "glm-5.2", None);
        let mut fut = std::pin::pin!(fut);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let std::task::Poll::Ready(r) = fut.as_mut().poll(&mut cx) else {
            panic!("the remote check must finish before any I/O");
        };
        assert!(matches!(r, Err(RuntimeError::Config(_))), "{r:?}");
    }
}
