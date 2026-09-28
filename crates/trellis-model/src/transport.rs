//! One HTTP/1.1 request over TCP, or over TLS with the `hosted` feature.
//!
//! Hand-written like `runtime.rs`, and for the same reason: a hosted API is
//! one POST with a JSON body, and an HTTP stack is a large dependency for
//! that. What this adds over the local client is what a hosted endpoint
//! needs: TLS (ADR 0024), response headers (`Retry-After`), and bodies
//! framed by `Content-Length` or chunked encoding, because a server behind a
//! load balancer does not always close the connection when it is done.

use crate::runtime::RuntimeError;
use crate::secret::ApiKey;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// Where a hosted API lives, parsed once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// The base URL's path, without a trailing slash: `/v1` for
    /// `https://api.openai.com/v1`, empty for `https://api.anthropic.com`.
    pub prefix: String,
}

impl Endpoint {
    /// Parses `http://` or `https://` URLs. Plain HTTP only to a loopback
    /// host: a key sent in the clear to anywhere else is a key disclosed.
    pub(crate) fn parse(base: &str) -> Result<Self, RuntimeError> {
        let (tls, rest) = if let Some(r) = base.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = base.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(RuntimeError::Config(format!(
                "{base} must start with https:// (or http:// for a server on this machine)"
            )));
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        let (host, port) = split_authority(base, authority, if tls { 443 } else { 80 })?;
        if !tls && !is_loopback(&host) {
            return Err(RuntimeError::Config(format!(
                "{base} is plain http to a host that is not this machine. A hosted \
                 provider is reached over https, so its key never travels in the clear."
            )));
        }
        Ok(Self {
            tls,
            host,
            port,
            prefix: path.trim_end_matches('/').to_string(),
        })
    }

    /// The `Host:` header's value.
    fn authority(&self) -> String {
        host_header(&self.host, self.port, if self.tls { 443 } else { 80 })
    }
}

/// `(host, port)` from a URL's authority: `name`, `name:port`, `[v6]` or
/// `[v6]:port`. The host is returned without brackets, as a TLS server name
/// and an IP parse both want it. Shared with the local runtime client, and
/// public so the binary's endpoint allow list reads an authority exactly as
/// the connection will: two parsers could approve one host and reach
/// another.
pub fn split_authority(
    base: &str,
    authority: &str,
    default_port: u16,
) -> Result<(String, u16), RuntimeError> {
    let port = |p: &str| {
        p.parse::<u16>()
            .map_err(|_| RuntimeError::Config(format!("{base}: `{p}` is not a port number")))
    };
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        // Split at the bracket, not the last colon: `[::1]` has three.
        let (h, after) = rest
            .split_once(']')
            .ok_or_else(|| RuntimeError::Config(format!("{base}: `[` without `]`")))?;
        let p = match after {
            "" => default_port,
            _ => port(after.strip_prefix(':').ok_or_else(|| {
                RuntimeError::Config(format!("{base}: `{after}` after `]` is not `:port`"))
            })?)?,
        };
        (h.to_string(), p)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), port(p)?),
            None => (authority.to_string(), default_port),
        }
    };
    if host.is_empty() {
        return Err(RuntimeError::Config(format!("{base} has no host")));
    }
    Ok((host, port))
}

/// A `Host:` value: brackets around an IPv6 literal, and the port when it
/// is not the scheme's default (RFC 9110 §7.2: a missing port means the
/// default, so leaving `:8080` off names another server).
pub(crate) fn host_header(host: &str, port: u16, default_port: u16) -> String {
    let h = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == default_port {
        h
    } else {
        format!("{h}:{port}")
    }
}

/// An address `TcpStream::connect` accepts: an IPv6 literal in brackets.
pub(crate) fn socket_addr(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Whether `host` (bracketed or not) is this machine: `localhost` or a
/// loopback address. The one test plain http and the allow list both use.
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// How a key is put on the wire. Two shapes cover both protocols.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Auth {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `x-api-key: <key>`.
    XApiKey,
}

/// A response, with the headers `Retry-After` needs.
#[derive(Debug)]
pub(crate) struct Response {
    pub status: u16,
    /// Lower-cased names.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Transport failures, split by whether retrying could help.
#[derive(Debug)]
pub(crate) enum SendError {
    /// Nothing is listening, or the name does not resolve. Retrying repeats it.
    Connect(String),
    /// The connection was made and then failed mid-request. Worth a retry.
    Broken(String),
    Timeout,
    Config(String),
}

/// The request line and the fixed headers, without the key.
fn request_head(ep: &Endpoint, path: &str, body_len: usize) -> String {
    format!(
        "POST {}{path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
         Accept: application/json\r\nContent-Length: {body_len}\r\nConnection: close\r\n",
        ep.prefix,
        ep.authority(),
    )
}

/// Sends one POST and reads the whole response.
///
/// The key is written into the request buffer here and nowhere else, and
/// the buffer is dropped when the write completes.
pub(crate) async fn post(
    ep: &Endpoint,
    path: &str,
    extra_headers: &[(&str, &str)],
    auth: Option<(Auth, &ApiKey)>,
    body: &str,
    timeout: Duration,
) -> Result<Response, SendError> {
    let mut req = request_head(ep, path, body.len());
    for (k, v) in extra_headers {
        req.push_str(k);
        req.push_str(": ");
        req.push_str(v);
        req.push_str("\r\n");
    }
    if let Some((how, key)) = auth {
        match how {
            Auth::Bearer => {
                req.push_str("Authorization: Bearer ");
            }
            Auth::XApiKey => {
                req.push_str("x-api-key: ");
            }
        }
        req.push_str(key.expose());
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    req.push_str(body);
    let bytes = req.into_bytes();

    let fut = async {
        let addr = socket_addr(&ep.host, ep.port);
        let tcp = TcpStream::connect(&addr)
            .await
            .map_err(|e| SendError::Connect(format!("cannot reach {addr}: {e}")))?;
        if ep.tls {
            tls_exchange(tcp, &ep.host, &bytes).await
        } else {
            exchange(tcp, &bytes).await
        }
    };
    tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| SendError::Timeout)?
}

#[cfg(feature = "hosted")]
async fn tls_exchange(tcp: TcpStream, host: &str, req: &[u8]) -> Result<Response, SendError> {
    let connector = tokio_rustls::TlsConnector::from(tls_config().map_err(SendError::Config)?);
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| SendError::Config(format!("`{host}` is not a TLS server name: {e}")))?;
    let stream = connector
        .connect(name, tcp)
        .await
        .map_err(|e| SendError::Broken(format!("TLS handshake with {host} failed: {e}")))?;
    exchange(stream, req).await
}

#[cfg(not(feature = "hosted"))]
async fn tls_exchange(_: TcpStream, _: &str, _: &[u8]) -> Result<Response, SendError> {
    Err(SendError::Config(NO_TLS.to_string()))
}

/// Said when an https base meets a build without TLS.
pub(crate) const NO_TLS: &str = "this trellis was built without the `hosted` feature, so it \
     has no TLS and cannot reach an https endpoint. Rebuild with \
     `--features hosted` (it is on in the default build).";

/// The client configuration: Mozilla's roots from `webpki-roots`, the
/// `ring` provider, and the provider's safe default protocol versions.
#[cfg(feature = "hosted")]
pub(crate) fn tls_config() -> Result<std::sync::Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| format!("TLS configuration failed: {e}"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(std::sync::Arc::new(config))
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    req: &[u8],
) -> Result<Response, SendError> {
    s.write_all(req)
        .await
        .map_err(|e| SendError::Broken(format!("sending the request failed: {e}")))?;
    s.flush()
        .await
        .map_err(|e| SendError::Broken(format!("sending the request failed: {e}")))?;
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    let mut framer = Framer::default();
    loop {
        match s.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if framer.complete(&raw) {
                    break;
                }
            }
            // A TLS peer that closes without close_notify reads as this. The
            // framing check below decides whether what arrived is whole.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(SendError::Broken(format!(
                    "reading the response failed: {e}"
                )));
            }
        }
    }
    parse_response(&raw).map_err(SendError::Broken)
}

/// Decides when a response read in pieces is whole, by its own framing,
/// resuming where the last call stopped.
///
/// Called after every read with the whole buffer so far. It used to
/// rescan all of it each time, for the header and then for the chunked
/// terminator, which made a large reply quadratic; and the terminator was
/// the first `0\r\n\r\n` anywhere in the body, which a chunk's size line
/// (`10\r\n`) or its data can contain. Now the header is found and parsed
/// once, and a chunked body is walked chunk by chunk: size line, data
/// skipped by its length, CRLF, until the zero-size chunk and its trailers.
#[derive(Default)]
struct Framer {
    /// Bytes already searched for the header's end.
    scanned: usize,
    body: Option<Body>,
    /// Bytes looked at, for the test that holds the work linear.
    examined: usize,
}

enum Body {
    /// `Content-Length`: whole at this many bytes of buffer.
    Length(usize),
    /// Neither: whole only at end of stream.
    Unframed,
    Chunked {
        pos: usize,
        at: Chunk,
    },
}

#[derive(Clone, Copy)]
enum Chunk {
    Size,
    Data(usize),
    DataEnd,
    Trailers,
}

/// A size line longer than this is not one; the parser reports it.
const MAX_SIZE_LINE: usize = 4096;

impl Framer {
    fn complete(&mut self, raw: &[u8]) -> bool {
        if self.body.is_none() {
            let from = self.scanned.saturating_sub(3);
            self.examined += raw.len() - from;
            let Some(i) = find(&raw[from..], b"\r\n\r\n") else {
                self.scanned = raw.len();
                return false;
            };
            let end = from + i + 4;
            let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
            self.body = Some(if head.contains("transfer-encoding: chunked") {
                Body::Chunked {
                    pos: end,
                    at: Chunk::Size,
                }
            } else {
                match content_length(&head) {
                    Some(n) => Body::Length(end.saturating_add(n)),
                    None => Body::Unframed,
                }
            });
        }
        match self.body.as_mut() {
            Some(Body::Length(n)) => raw.len() >= *n,
            Some(Body::Chunked { pos, at }) => chunked(raw, pos, at, &mut self.examined),
            Some(Body::Unframed) | None => false,
        }
    }
}

/// Advances through a chunked body from `pos`. True once the zero-size
/// chunk and the blank line after its trailers have arrived, or once the
/// framing is broken — then the parser, which is stricter, says how.
fn chunked(raw: &[u8], pos: &mut usize, at: &mut Chunk, examined: &mut usize) -> bool {
    loop {
        let rest = &raw[*pos..];
        match *at {
            Chunk::Size | Chunk::Trailers => {
                let window = &rest[..rest.len().min(MAX_SIZE_LINE + 2)];
                *examined += window.len();
                let Some(eol) = find(window, b"\r\n") else {
                    return rest.len() > MAX_SIZE_LINE;
                };
                *pos += eol + 2;
                if let Chunk::Trailers = at {
                    if eol == 0 {
                        return true;
                    }
                    continue;
                }
                match chunk_size(&rest[..eol]) {
                    Ok(0) => *at = Chunk::Trailers,
                    Ok(n) => *at = Chunk::Data(n),
                    Err(_) => return true,
                }
            }
            Chunk::Data(n) => {
                if rest.len() < n {
                    *pos += rest.len();
                    *at = Chunk::Data(n - rest.len());
                    return false;
                }
                *pos += n;
                *at = Chunk::DataEnd;
            }
            Chunk::DataEnd => {
                if rest.len() < 2 {
                    return false;
                }
                *examined += 2;
                if &rest[..2] != b"\r\n" {
                    return true;
                }
                *pos += 2;
                *at = Chunk::Size;
            }
        }
    }
}

/// A chunk-size line's size: hex digits, then optional `;extensions`.
fn chunk_size(line: &[u8]) -> Result<usize, String> {
    let text = String::from_utf8_lossy(line);
    let hex = text.split(';').next().unwrap_or("").trim();
    usize::from_str_radix(hex, 16).map_err(|_| format!("`{hex}` is not a chunk size"))
}

/// Whether `raw` holds a whole response by its own framing. One pass; for
/// a response read in pieces `exchange` keeps a [`Framer`] instead.
#[cfg(test)]
fn complete(raw: &[u8]) -> bool {
    Framer::default().complete(raw)
}

fn content_length(head_lower: &str) -> Option<usize> {
    head_lower
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse().ok())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub(crate) fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let end = find(raw, b"\r\n\r\n").ok_or("no HTTP header terminator in the response")?;
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no HTTP status in `{status_line}`"))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let body_raw = &raw[end + 4..];
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let body = if chunked {
        dechunk(body_raw)?
    } else {
        match headers
            .iter()
            .find(|(k, _)| k == "content-length")
            .and_then(|(_, v)| v.parse::<usize>().ok())
        {
            Some(n) if body_raw.len() >= n => body_raw[..n].to_vec(),
            Some(n) => {
                return Err(format!(
                    "the response was cut short: {} of {n} body bytes arrived",
                    body_raw.len()
                ));
            }
            None => body_raw.to_vec(),
        }
    };
    Ok(Response {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = find(b, b"\r\n").ok_or("a chunk size line is unterminated")?;
        let size = chunk_size(&b[..line_end])?;
        b = &b[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size {
            return Err("the response was cut short inside a chunk".into());
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size..];
        b = b.strip_prefix(b"\r\n").unwrap_or(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_http_endpoint_is_accepted_only_on_this_machine() {
        // The risk: a key written into a header over plain HTTP to a remote
        // host is readable by anything on the path.
        assert!(Endpoint::parse("http://127.0.0.1:8080/v1").is_ok());
        assert!(Endpoint::parse("http://localhost:1234").is_ok());
        assert!(Endpoint::parse("http://[::1]:9/v1").is_ok());
        let e = Endpoint::parse("http://api.openai.com/v1").unwrap_err();
        assert!(e.to_string().contains("https"), "{e}");
    }

    #[test]
    fn an_https_base_keeps_its_path_and_defaults_the_port() {
        let ep = Endpoint::parse("https://api.openai.com/v1/").unwrap();
        assert_eq!(
            ep,
            Endpoint {
                tls: true,
                host: "api.openai.com".into(),
                port: 443,
                prefix: "/v1".into(),
            }
        );
        let ep = Endpoint::parse("https://api.anthropic.com").unwrap();
        assert_eq!(ep.prefix, "");
    }

    #[test]
    fn a_bracketed_ipv6_host_parses_with_and_without_a_port() {
        // The risk: `rsplit_once(':')` split `[::1]` itself, so
        // `http://[::1]/v1` was refused as having port `1]`.
        let ep = Endpoint::parse("http://[::1]/v1").expect("no port");
        assert_eq!((ep.host.as_str(), ep.port), ("::1", 80));
        let ep = Endpoint::parse("http://[::1]:9/v1").expect("a port");
        assert_eq!(
            (ep.host.as_str(), ep.port, ep.prefix.as_str()),
            ("::1", 9, "/v1")
        );
        let ep = Endpoint::parse("https://[2001:db8::1]").expect("https");
        assert_eq!((ep.host.as_str(), ep.port), ("2001:db8::1", 443));
        for bad in [
            "http://[::1",
            "http://[::1]x/v1",
            "http://[]:9",
            "http://[::1]:x",
        ] {
            assert!(Endpoint::parse(bad).is_err(), "{bad}");
        }
        // Still only loopback over plain http.
        assert!(Endpoint::parse("http://[2001:db8::1]/v1").is_err());
    }

    #[test]
    fn the_host_header_names_a_port_that_is_not_the_scheme_default() {
        // The risk: `Host:` carried the name alone, so a server on
        // `127.0.0.1:8080` behind a name-based proxy was sent `Host:
        // 127.0.0.1`, which RFC 9110 §7.2 says names port 80.
        let host = |base: &str| {
            let head = request_head(&Endpoint::parse(base).unwrap(), "/x", 0);
            head.lines()
                .find_map(|l| l.strip_prefix("Host: ").map(str::to_string))
                .unwrap()
        };
        assert_eq!(host("http://127.0.0.1:8080/v1"), "127.0.0.1:8080");
        assert_eq!(host("http://localhost/v1"), "localhost");
        assert_eq!(host("https://api.openai.com/v1"), "api.openai.com");
        assert_eq!(host("https://api.example.com:8443"), "api.example.com:8443");
        assert_eq!(host("http://[::1]/v1"), "[::1]");
        assert_eq!(host("http://[::1]:9/v1"), "[::1]:9");
    }

    #[test]
    fn chunked_bodies_are_reassembled() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert!(complete(raw));
        let r = parse_response(raw).unwrap();
        assert_eq!(r.body, "hello world");
    }

    #[test]
    fn a_content_length_body_is_complete_without_eof() {
        // The server may keep the socket open; the length says when to stop.
        let raw =
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3\r\nContent-Length: 2\r\n\r\n{}";
        assert!(complete(raw));
        let r = parse_response(raw).unwrap();
        assert_eq!(r.status, 429);
        assert_eq!(r.header("retry-after"), Some("3"));
        assert!(!complete(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n{}"));
    }

    /// Feeds `raw` to a fresh framer the way `exchange` does, `step` bytes
    /// per read, and returns at which length it first said complete.
    fn feed(raw: &[u8], step: usize) -> (Option<usize>, usize) {
        let mut f = Framer::default();
        let mut n = 0;
        while n < raw.len() {
            n = (n + step).min(raw.len());
            if f.complete(&raw[..n]) {
                return (Some(n), f.examined);
            }
        }
        (None, f.examined)
    }

    #[test]
    fn a_large_response_is_framed_in_work_linear_in_its_size() {
        // The risk: every 16 KiB read rescanned the whole buffer for the
        // header, then the body for the terminator: a 4 MiB reply cost
        // about 256 * 2 MiB of scanning, quadratic in the reply.
        let body = vec![b'x'; 4 << 20];
        let mut cl =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        cl.extend_from_slice(&body);
        let (at, examined) = feed(&cl, 16 * 1024);
        assert_eq!(at, Some(cl.len()));
        assert!(
            examined < 2 * cl.len(),
            "examined {examined} for {}",
            cl.len()
        );

        let mut ch = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for piece in body.chunks(8 * 1024) {
            ch.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
            ch.extend_from_slice(piece);
            ch.extend_from_slice(b"\r\n");
        }
        ch.extend_from_slice(b"0\r\n\r\n");
        let (at, examined) = feed(&ch, 16 * 1024);
        assert_eq!(at, Some(ch.len()));
        assert!(
            examined < 2 * ch.len(),
            "examined {examined} for {}",
            ch.len()
        );
        assert_eq!(parse_response(&ch).unwrap().body.len(), body.len());
    }

    #[test]
    fn a_zero_inside_a_chunk_is_not_the_terminator() {
        let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        // A size line that ends in 0, and data that starts with CRLF: the
        // first `0\r\n\r\n` in the body is inside `10\r\n\r\n…`.
        let mut raw = head.clone();
        raw.extend_from_slice(b"10\r\n\r\n0\r\n\r\nabcdefghi\r\n0\r\n\r\n");
        assert_eq!(
            feed(&raw, 1).0,
            Some(raw.len()),
            "complete, and only at the end"
        );
        assert_eq!(parse_response(&raw).unwrap().body, "\r\n0\r\n\r\nabcdefghi");

        // Data that is itself `0\r\n\r\n…` right after a size line: not the
        // end until the chunk's 7 bytes and the real terminator arrive.
        let mut raw = head;
        raw.extend_from_slice(b"7\r\n0\r\n\r\nab\r\n0\r\n\r\n");
        assert_eq!(feed(&raw, 1).0, Some(raw.len()));
        assert_eq!(parse_response(&raw).unwrap().body, "0\r\n\r\nab");
    }

    #[test]
    fn a_short_body_is_an_error_not_a_truncated_success() {
        let e = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}").unwrap_err();
        assert!(e.contains("cut short"), "{e}");
    }

    #[cfg(feature = "hosted")]
    #[test]
    fn the_tls_configuration_builds_with_the_compiled_in_roots() {
        // A provider or root-set mismatch would otherwise surface as a failed
        // handshake on a user's first hosted run.
        tls_config().expect("the ring provider and webpki roots configure");
    }
}
