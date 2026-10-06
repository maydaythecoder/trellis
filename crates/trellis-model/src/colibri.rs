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
            SendError::Connect(m) | SendError::ConnectTimedOut(m) | SendError::Broken(m) => {
                RuntimeError::Connect {
                    base: base.to_string(),
                    source: std::io::Error::other(m),
                }
            }
            SendError::Timeout => RuntimeError::Timeout(READ_TIMEOUT),
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
                let mut buf = vec![0u8; 65536];
                let n = s.read(&mut buf).await.unwrap();
                got.push(String::from_utf8_lossy(&buf[..n]).into_owned());
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
}
