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

/// An HTTP proxy that `https://` requests are tunnelled through with
/// `CONNECT` (ADR 0024, amended by #150).
///
/// Only a tunnel: the proxy is told `host:port` and then carries TLS it
/// cannot read, so the key and the transcript stay between trellis and the
/// provider. Plain `http://` is never sent through it, because a proxy that
/// forwards plain requests reads them, and plain http goes only to this
/// machine anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proxy {
    pub host: String,
    pub port: u16,
}

impl std::fmt::Display for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&socket_addr(&self.host, self.port))
    }
}

/// The first non-empty value among `names`. Empty counts as unset, as
/// shells export `HTTPS_PROXY=` to turn a proxy off.
fn first_set(env: &dyn Fn(&str) -> Option<String>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|n| env(n))
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

/// The proxy `ep` is reached through, if any, reading variables with `env`.
///
/// `None` for plain http, for a loopback host, and for a host `NO_PROXY`
/// names; only then is `HTTPS_PROXY` read at all, so a proxy URL this
/// refuses breaks only the requests that would have used it. `HTTP_PROXY`
/// and `ALL_PROXY` are not read: the first names a proxy for plain http,
/// which is never proxied, and the second is as often a SOCKS proxy, which
/// this does not speak.
pub(crate) fn proxy_for(
    ep: &Endpoint,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Proxy>, RuntimeError> {
    if !ep.tls || is_loopback(&ep.host) {
        return Ok(None);
    }
    if first_set(env, &["NO_PROXY", "no_proxy"])
        .is_some_and(|list| no_proxy_matches(&list, &ep.host))
    {
        return Ok(None);
    }
    match first_set(env, &["HTTPS_PROXY", "https_proxy"]) {
        Some(url) => parse_proxy(&url).map(Some),
        None => Ok(None),
    }
}

/// Whether a `NO_PROXY` list names `host`: comma-separated entries, `*` for
/// every host, `.suffix` for the domain and every name under it, and a bare
/// name for itself and every name under it (curl's and Go's reading). Names
/// compare without case and IPv6 literals without brackets. An entry with a
/// `:port` or a CIDR range is compared as written, so it never matches a
/// host and the request goes through the proxy.
fn no_proxy_matches(list: &str, host: &str) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    list.split(',')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .any(|e| {
            if e == "*" {
                return true;
            }
            let name = e
                .trim_start_matches('.')
                .trim_start_matches('[')
                .trim_end_matches(']');
            !name.is_empty()
                && (host == name
                    || host
                        .strip_suffix(name)
                        .is_some_and(|rest| rest.ends_with('.')))
        })
}

/// Parses `HTTPS_PROXY`: `http://host:port`, `host:port`, or either without
/// a port (80). Messages never repeat the value: a URL with credentials in
/// it would print them.
fn parse_proxy(url: &str) -> Result<Proxy, RuntimeError> {
    let rest = match url.split_once("://") {
        None => url,
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
        Some((scheme, _)) => {
            return Err(RuntimeError::Config(format!(
                "HTTPS_PROXY names a `{scheme}://` proxy. Only an http proxy that \
                 accepts CONNECT is supported: set HTTPS_PROXY=http://host:port, or \
                 unset it to connect directly."
            )));
        }
    };
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') {
        return Err(RuntimeError::Config(
            "HTTPS_PROXY carries a user name or password (`user:pass@`). Proxy \
             credentials are not supported, and none were sent: use a proxy that \
             does not need them, or add the provider's host to NO_PROXY."
                .to_string(),
        ));
    }
    let (host, port) = split_authority("HTTPS_PROXY", authority, 80)?;
    Ok(Proxy { host, port })
}

/// Opens a `CONNECT` tunnel to `target` through `proxy`.
///
/// The response head is read a byte at a time: whatever follows it belongs
/// to the tunnel, and a buffered read could take some of it. A 200 is the
/// only answer accepted; anything else is the proxy refusing, which a retry
/// would repeat.
pub(crate) async fn connect_tunnel(
    proxy: &Proxy,
    target: &Endpoint,
) -> Result<TcpStream, SendError> {
    let mut s = TcpStream::connect(proxy.to_string())
        .await
        .map_err(|e| SendError::Connect(format!("cannot reach the proxy at {proxy}: {e}")))?;
    let dest = socket_addr(&target.host, target.port);
    tracing::debug!(%proxy, %dest, "opening a CONNECT tunnel");
    s.write_all(format!("CONNECT {dest} HTTP/1.1\r\nHost: {dest}\r\n\r\n").as_bytes())
        .await
        .map_err(|e| {
            SendError::Broken(format!(
                "sending CONNECT to the proxy at {proxy} failed: {e}"
            ))
        })?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_CONNECT_HEAD {
            return Err(SendError::Broken(format!(
                "the proxy at {proxy} sent a CONNECT response head over {MAX_CONNECT_HEAD} bytes"
            )));
        }
        match s.read(&mut byte).await {
            Ok(0) => {
                return Err(SendError::Broken(format!(
                    "the proxy at {proxy} closed the connection before answering CONNECT"
                )));
            }
            Ok(_) => head.push(byte[0]),
            Err(e) => {
                return Err(SendError::Broken(format!(
                    "reading the proxy's CONNECT response failed: {e}"
                )));
            }
        }
    }
    let status = parse_head(&head[..head.len() - 4])
        .map_err(|e| SendError::Broken(format!("the proxy at {proxy} answered CONNECT with {e}")))?
        .status;
    match status {
        200 => Ok(s),
        407 => Err(SendError::Connect(format!(
            "the proxy at {proxy} asks for credentials (407) to reach {dest}; proxy \
             credentials are not supported"
        ))),
        n => Err(SendError::Connect(format!(
            "the proxy at {proxy} refused CONNECT to {dest} with status {n}"
        ))),
    }
}

/// A CONNECT response is a status line and a few headers.
const MAX_CONNECT_HEAD: usize = 16 * 1024;

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
/// the buffer is dropped when the write completes. With a `proxy` it is
/// written inside the TLS session the tunnel carries, never to the proxy.
pub(crate) async fn post(
    ep: &Endpoint,
    proxy: Option<&Proxy>,
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
        let tcp = match proxy {
            // `proxy_for` returns one only for https, so the plain branch
            // below never runs over a tunnel; checked again here because
            // what reaches a proxy in the clear is the whole request.
            Some(p) if ep.tls => connect_tunnel(p, ep).await?,
            Some(_) => {
                return Err(SendError::Config(
                    "plain http is never sent through a proxy".to_string(),
                ));
            }
            None => {
                let addr = socket_addr(&ep.host, ep.port);
                TcpStream::connect(&addr)
                    .await
                    .map_err(|e| SendError::Connect(format!("cannot reach {addr}: {e}")))?
            }
        };
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
    let too_large = || {
        SendError::Broken(format!(
            "the response is larger than {} MiB, which no model reply is; nothing more was read",
            MAX_RESPONSE >> 20
        ))
    };
    loop {
        match s.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if raw.len() + n > MAX_RESPONSE {
                    return Err(too_large());
                }
                raw.extend_from_slice(&buf[..n]);
                if framer.complete(&raw) {
                    break;
                }
                // A declared length past the cap is refused on the header,
                // not after the bytes it promises.
                if framer.declared().is_some_and(|n| n > MAX_RESPONSE) {
                    return Err(too_large());
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

/// The most a response may be, head and body: 32 MiB. A model reply is
/// kilobytes, and a whole large-context completion well under this; past
/// it, the server is not answering the request that was sent, and
/// reading on would fill memory until the timeout.
pub(crate) const MAX_RESPONSE: usize = 32 << 20;

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
            // The parser's own reading of the header, so the two cannot
            // disagree about where the body ends.
            let Ok(head) = parse_head(&raw[..end - 4]) else {
                // Not a response header: the parser says why.
                return true;
            };
            self.body = Some(match head.framing {
                Framing::Chunked => Body::Chunked {
                    pos: end,
                    at: Chunk::Size,
                },
                Framing::Length(n) => Body::Length(end.saturating_add(n)),
                Framing::Unframed => Body::Unframed,
            });
        }
        match self.body.as_mut() {
            Some(Body::Length(n)) => raw.len() >= *n,
            Some(Body::Chunked { pos, at }) => chunked(raw, pos, at, &mut self.examined),
            Some(Body::Unframed) | None => false,
        }
    }

    /// The whole response's length in bytes when its header declared one.
    fn declared(&self) -> Option<usize> {
        match self.body {
            Some(Body::Length(n)) => Some(n),
            _ => None,
        }
    }
}

/// How a response's body ends.
#[derive(Debug, PartialEq, Eq)]
enum Framing {
    Chunked,
    Length(usize),
    /// At the end of the stream.
    Unframed,
}

/// A response header, parsed once for the framer and the parser alike.
struct Head {
    status: u16,
    /// Lower-cased names, trimmed values.
    headers: Vec<(String, String)>,
    framing: Framing,
}

/// Parses the status line and the header fields (the bytes before the
/// blank line). Field names compare case-insensitively; values are
/// trimmed. The framing is RFC 9112 §6.3's: any `Transfer-Encoding`
/// overrides `Content-Length`; the body is chunked if and only if the last
/// coding of the combined list (every `Transfer-Encoding` field, each a
/// comma-separated list) is `chunked`, and otherwise runs to the end of the
/// stream; with no `Transfer-Encoding`, a valid `Content-Length` frames it.
fn parse_head(head: &[u8]) -> Result<Head, String> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
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
    let codings: Vec<String> = headers
        .iter()
        .filter(|(k, _)| k == "transfer-encoding")
        .flat_map(|(_, v)| v.split(','))
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty())
        .collect();
    let framing = if let Some(last) = codings.last() {
        if last == "chunked" {
            Framing::Chunked
        } else {
            Framing::Unframed
        }
    } else {
        match headers
            .iter()
            .find(|(k, _)| k == "content-length")
            .and_then(|(_, v)| v.parse::<usize>().ok())
        {
            Some(n) => Framing::Length(n),
            None => Framing::Unframed,
        }
    };
    Ok(Head {
        status,
        headers,
        framing,
    })
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

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub(crate) fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let end = find(raw, b"\r\n\r\n").ok_or("no HTTP header terminator in the response")?;
    let Head {
        status,
        headers,
        framing,
    } = parse_head(&raw[..end])?;
    let body_raw = &raw[end + 4..];
    let body = match framing {
        Framing::Chunked => dechunk(body_raw)?,
        Framing::Length(n) if body_raw.len() >= n => body_raw[..n].to_vec(),
        Framing::Length(n) => {
            return Err(format!(
                "the response was cut short: {} of {n} body bytes arrived",
                body_raw.len()
            ));
        }
        Framing::Unframed => body_raw.to_vec(),
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
    fn the_framer_and_the_parser_read_transfer_encoding_the_same_way() {
        // The risk: the framer looked for the substring
        // `transfer-encoding: chunked` and the parser for a header named
        // `transfer-encoding` containing `chunked`, so `Transfer-Encoding:
        // chunked` without the space was read to EOF and then dechunked,
        // and `x-transfer-encoding: chunked` was framed as chunked and
        // then parsed by its length (review round 8, finding 6).
        let body = "5\r\nhello\r\n0\r\n\r\n";
        for (headers, chunked) in [
            ("Transfer-Encoding:chunked", true),
            ("transfer-encoding:   Chunked  ", true),
            ("Transfer-Encoding: gzip, chunked", true),
            (
                "Transfer-Encoding: gzip\r\nTransfer-Encoding: chunked",
                true,
            ),
            (
                "Content-Length: 3\r\nTransfer-Encoding: chunked",
                true, // chunked overrides the length
            ),
            ("x-transfer-encoding: chunked\r\nContent-Length: 15", false),
        ] {
            let raw = format!("HTTP/1.1 200 OK\r\n{headers}\r\n\r\n{body}").into_bytes();
            assert_eq!(feed(&raw, 1).0, Some(raw.len()), "framer: {headers}");
            let r = parse_response(&raw).unwrap();
            let want = if chunked { "hello" } else { body };
            assert_eq!(r.body, want, "parser: {headers}");
        }
        // Another header's value is not a transfer coding: the length
        // frames it, and the framer stops where the parser cuts.
        let head = "HTTP/1.1 200 OK\r\nx-transfer-encoding: chunked\r\nContent-Length: 3\r\n\r\n";
        let raw = format!("{head}{body}").into_bytes();
        assert_eq!(feed(&raw, 1).0, Some(head.len() + 3));
        assert_eq!(parse_response(&raw).unwrap().body, "5\r\n");
        // A last coding that is not `chunked` is read to the end of the
        // stream, whatever `Content-Length` says.
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, gzip\r\nContent-Length: 2\r\n\r\n{}xyz";
        assert_eq!(feed(raw, 1).0, None, "only EOF ends it");
        assert_eq!(parse_response(raw).unwrap().body, "{}xyz");
    }

    /// A loopback server that answers one connection with `head` and then
    /// `body_bytes` bytes of `x`, ignoring errors (the client may hang up).
    async fn stub(head: &'static str, body_bytes: usize) -> TcpStream {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut req = [0u8; 1024];
            let _ = s.read(&mut req).await;
            let _ = s.write_all(head.as_bytes()).await;
            let chunk = vec![b'x'; 1 << 20];
            let mut sent = 0;
            while sent < body_bytes {
                if s.write_all(&chunk).await.is_err() {
                    return;
                }
                sent += chunk.len();
            }
            // Held open: a client that waits for EOF waits for the timeout.
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        TcpStream::connect(addr).await.unwrap()
    }

    #[tokio::test]
    async fn a_response_past_the_cap_is_refused_rather_than_buffered() {
        // The risk: `raw` grew without bound, so a server streaming an
        // unframed body filled memory until the request's timeout
        // (review round 8, finding 8).
        let s = stub("HTTP/1.1 200 OK\r\n\r\n", MAX_RESPONSE + (4 << 20)).await;
        let r = tokio::time::timeout(
            Duration::from_secs(20),
            exchange(s, b"GET / HTTP/1.1\r\n\r\n"),
        )
        .await
        .expect("refused at the cap, not at the end of the stream");
        match r {
            Err(SendError::Broken(e)) => assert!(e.contains("32 MiB"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_declared_length_past_the_cap_is_refused_before_the_body() {
        let s = stub("HTTP/1.1 200 OK\r\nContent-Length: 1073741824\r\n\r\n", 0).await;
        let r = tokio::time::timeout(
            Duration::from_secs(10),
            exchange(s, b"GET / HTTP/1.1\r\n\r\n"),
        )
        .await
        .expect("refused on the header");
        match r {
            Err(SendError::Broken(e)) => assert!(e.contains("32 MiB"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_short_body_is_an_error_not_a_truncated_success() {
        let e = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}").unwrap_err();
        assert!(e.contains("cut short"), "{e}");
    }

    /// A process environment with only `vars` set.
    fn env(vars: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let vars = vars.to_vec();
        move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    fn proxied(base: &str, vars: &[(&'static str, &'static str)]) -> Option<Proxy> {
        proxy_for(&Endpoint::parse(base).unwrap(), &env(vars)).unwrap()
    }

    #[test]
    fn a_proxy_is_used_only_for_https_to_a_remote_host() {
        // The risk: plain http through a proxy is readable by the proxy, key
        // header included; and a local server reached through a remote
        // proxy is not the local server.
        let p = [("HTTPS_PROXY", "http://proxy.example:3128")];
        assert_eq!(
            proxied("https://api.openai.com/v1", &p),
            Some(Proxy {
                host: "proxy.example".into(),
                port: 3128
            })
        );
        assert_eq!(proxied("http://127.0.0.1:8080/v1", &p), None);
        assert_eq!(proxied("https://localhost:8443/v1", &p), None);
        assert_eq!(proxied("https://[::1]:8443/v1", &p), None);
        // Only the https variables are read.
        for other in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
            let e: [(&'static str, &'static str); 1] = [(other, "http://proxy.example:3128")];
            assert_eq!(proxied("https://api.openai.com/v1", &e), None, "{other}");
        }
        assert_eq!(
            proxied(
                "https://api.openai.com/v1",
                &[("https_proxy", "proxy.example:8080")]
            ),
            Some(Proxy {
                host: "proxy.example".into(),
                port: 8080
            })
        );
        assert_eq!(
            proxied("https://api.openai.com/v1", &[("HTTPS_PROXY", "  ")]),
            None,
            "empty is unset"
        );
        assert_eq!(
            proxied(
                "https://api.openai.com/v1",
                &[("HTTPS_PROXY", "http://p.example/")]
            ),
            Some(Proxy {
                host: "p.example".into(),
                port: 80
            })
        );
    }

    #[test]
    fn no_proxy_names_hosts_suffixes_and_everything() {
        let base = "https://api.openai.com/v1";
        let with = |list: &'static str| {
            proxied(
                base,
                &[
                    ("HTTPS_PROXY", "http://proxy.example:3128"),
                    ("NO_PROXY", list),
                ],
            )
        };
        for direct in [
            "api.openai.com",
            "API.OpenAI.com",
            "openai.com",
            ".openai.com",
            "localhost, .openai.com",
            "*",
        ] {
            assert_eq!(with(direct), None, "{direct}");
        }
        for through in [
            "notopenai.com",
            "i.com",
            ".api.openai.com.evil",
            "other.example,localhost",
            // Ports and CIDR ranges are not read, so they do not match.
            "api.openai.com:443",
            "",
        ] {
            assert!(with(through).is_some(), "{through}");
        }
        assert_eq!(
            proxied(
                base,
                &[
                    ("HTTPS_PROXY", "http://proxy.example:3128"),
                    ("no_proxy", "openai.com")
                ]
            ),
            None
        );
        assert_eq!(
            proxied(
                "https://[2001:db8::1]/v1",
                &[
                    ("HTTPS_PROXY", "http://proxy.example:3128"),
                    ("NO_PROXY", "[2001:db8::1]")
                ]
            ),
            None
        );
    }

    #[test]
    fn a_proxy_with_credentials_is_refused_without_repeating_them() {
        // The risk: credentials either sent somewhere the user did not
        // expect, or printed in the refusal that names the problem.
        let ep = Endpoint::parse("https://api.openai.com/v1").unwrap();
        let e = proxy_for(
            &ep,
            &env(&[("HTTPS_PROXY", "http://alice:s3cret-pw@proxy.example:3128")]),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("credentials"), "{e}");
        assert!(!e.contains("s3cret-pw") && !e.contains("alice"), "{e}");
        // A host NO_PROXY exempts never reads the proxy, so a proxy URL it
        // would refuse does not break it.
        assert_eq!(
            proxy_for(
                &ep,
                &env(&[
                    ("HTTPS_PROXY", "http://alice:pw@proxy.example:3128"),
                    ("NO_PROXY", "openai.com")
                ])
            )
            .unwrap(),
            None
        );
        let e = proxy_for(&ep, &env(&[("HTTPS_PROXY", "socks5://proxy.example:1080")]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("socks5://") && e.contains("CONNECT"), "{e}");
        assert!(proxy_for(&ep, &env(&[("HTTPS_PROXY", "https://proxy.example:443")])).is_err());
    }

    /// A loopback proxy that answers one CONNECT with `status`, returns the
    /// head it received, then echoes what the tunnel carries.
    async fn connect_stub(status: &'static str) -> (Proxy, tokio::task::JoinHandle<String>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if s.read(&mut b).await.unwrap() == 0 {
                    break;
                }
                head.push(b[0]);
            }
            s.write_all(format!("HTTP/1.1 {status}\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut buf = [0u8; 64];
            if let Ok(n) = s.read(&mut buf).await {
                let _ = s.write_all(&buf[..n]).await;
            }
            String::from_utf8_lossy(&head).into_owned()
        });
        (
            Proxy {
                host: "127.0.0.1".into(),
                port,
            },
            h,
        )
    }

    #[tokio::test]
    async fn a_connect_tunnel_names_the_target_and_carries_bytes_after_200() {
        let (p, h) = connect_stub("200 Connection established").await;
        let target = Endpoint::parse("https://api.openai.com/v1").unwrap();
        let mut s = connect_tunnel(&p, &target).await.expect("tunnel");
        s.write_all(b"ping").await.unwrap();
        let mut got = [0u8; 4];
        s.read_exact(&mut got).await.unwrap();
        assert_eq!(
            &got, b"ping",
            "nothing of the tunnel was eaten by the head read"
        );
        let head = h.await.unwrap();
        assert!(
            head.starts_with("CONNECT api.openai.com:443 HTTP/1.1\r\n"),
            "{head}"
        );
        assert!(head.contains("Host: api.openai.com:443\r\n"), "{head}");
        assert!(
            !head.to_ascii_lowercase().contains("proxy-authorization"),
            "{head}"
        );
    }

    #[tokio::test]
    async fn a_proxy_that_refuses_connect_is_a_connect_error_not_a_retry() {
        let target = Endpoint::parse("https://api.openai.com/v1").unwrap();
        for (status, says) in [
            ("407 Proxy Authentication Required", "credentials"),
            ("403 Forbidden", "403"),
        ] {
            let (p, _h) = connect_stub(status).await;
            match connect_tunnel(&p, &target).await {
                Err(SendError::Connect(e)) => assert!(e.contains(says), "{e}"),
                other => panic!("{status}: {other:?}"),
            }
        }
    }

    #[cfg(feature = "hosted")]
    #[test]
    fn the_tls_configuration_builds_with_the_compiled_in_roots() {
        // A provider or root-set mismatch would otherwise surface as a failed
        // handshake on a user's first hosted run.
        tls_config().expect("the ring provider and webpki roots configure");
    }
}
