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
use std::time::{Duration, Instant};
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
        .map_err(|e| connect_error(e, format!("cannot reach the proxy at {proxy}")))?;
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
#[derive(Debug, thiserror::Error)]
pub(crate) enum SendError {
    /// Nothing is listening, the name does not resolve, or a proxy refused
    /// the tunnel. Retrying repeats it.
    #[error("{0}")]
    Connect(String),
    /// The operating system gave up on the TCP handshake (`TimedOut`; on
    /// Windows `os error 10060`, WSAETIMEDOUT). Nothing answered the SYN in
    /// time, which a loaded host produces even on loopback (#210), and a
    /// later attempt can get past. Worth a retry, where a refusal is not.
    #[error("{0}")]
    ConnectTimedOut(String),
    /// The connection was made and then failed mid-request. Worth a retry.
    #[error("{0}")]
    Broken(String),
    /// The whole request outran the client's timeout; for a streamed
    /// request, the server was silent for that long (#280).
    #[error("the request timed out")]
    Timeout,
    /// A streamed request reached the deadline its caller set, however
    /// recently the server last sent something (#280).
    #[error("the request reached its deadline")]
    Deadline,
    /// A streamed reply's events were not what the protocol sends. Not
    /// retried, as a whole reply that is not JSON is not: the server
    /// answered, and asking again regenerates the reply to the same end.
    #[error("{0}")]
    Stream(String),
    #[error("{0}")]
    Config(String),
}

/// The error for a response that is not a success. A 401 or 403 carries
/// its status and the key's variable and never the body, which may echo a
/// masked form of the key (ADR 0024 §3); any other body is scrubbed of the
/// key and cut to 400 characters.
pub(crate) fn status_error(r: &Response, key: Option<&ApiKey>) -> RuntimeError {
    if matches!(r.status, 401 | 403) {
        return RuntimeError::Auth {
            status: r.status,
            source_var: key.map_or_else(|| "no key".to_string(), |k| k.source().to_string()),
        };
    }
    let body = key.map_or_else(|| r.body.clone(), |k| k.scrub(&r.body));
    RuntimeError::Http {
        status: r.status,
        body: excerpt(&body, ERROR_BODY_CHARS),
    }
}

/// How many characters of an error body an `Http` error or a shape error
/// quotes: the whole of a reply up to the size cap is not a message anyone
/// reads.
pub(crate) const ERROR_BODY_CHARS: usize = 400;

/// How many characters of a malformed reply a hosted shape error quotes.
pub(crate) const SHAPE_QUOTE_CHARS: usize = 200;

/// The first `max_chars` characters of `s`, with no marker for what was
/// left out. Counted in characters, not bytes, so a cut never lands inside
/// a multi-byte character.
pub(crate) fn excerpt(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// A failed TCP connect as a `SendError`, `what` naming where it was going.
///
/// The connect has no timeout of its own: it runs inside the request's
/// timeout (`post`'s `timeout`, 300 s by default), so how long a handshake
/// may take is the operating system's to decide. Only its `TimedOut` is
/// kept apart, as the one kind a retry can get past; a refusal, a name
/// that does not resolve and the rest stay `Connect`, which is not retried
/// (ADR 0024 §4, amended by #210).
fn connect_error(e: std::io::Error, what: String) -> SendError {
    if e.kind() == std::io::ErrorKind::TimedOut {
        SendError::ConnectTimedOut(format!("{what}: {e}"))
    } else {
        SendError::Connect(format!("{what}: {e}"))
    }
}

/// The request line and the fixed headers, without the key.
fn request_head(ep: &Endpoint, path: &str, body_len: usize, accept: &str) -> String {
    format!(
        "POST {}{path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
         Accept: {accept}\r\nContent-Length: {body_len}\r\nConnection: close\r\n",
        ep.prefix,
        ep.authority(),
    )
}

/// Sends one POST and reads the whole response.
///
/// The key is written into the request buffer here and nowhere else, and
/// the buffer is dropped when `post` returns. With a `proxy` it is
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
    let mut req = request_head(ep, path, body.len(), "application/json");
    for (k, v) in extra_headers {
        req.push_str(k);
        req.push_str(": ");
        req.push_str(v);
        req.push_str("\r\n");
    }
    push_auth(&mut req, auth);
    req.push_str("\r\n");
    req.push_str(body);
    send(ep, proxy, req.into_bytes(), timeout).await
}

/// Sends one GET and reads the whole response, under the same size cap
/// and `timeout` as [`post`]. A redirect is returned as the response it
/// is and never followed, so nothing is sent to a host the caller did not
/// name.
///
/// Never through a proxy: its callers read loopback servers, which
/// `proxy_for` never proxies. The key header is written only when `auth`
/// is given.
pub(crate) async fn get(
    ep: &Endpoint,
    path: &str,
    auth: Option<(Auth, &ApiKey)>,
    timeout: Duration,
) -> Result<Response, SendError> {
    let mut req = format!(
        "GET {}{path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n",
        ep.prefix,
        ep.authority(),
    );
    push_auth(&mut req, auth);
    req.push_str("\r\n");
    send(ep, None, req.into_bytes(), timeout).await
}

/// Writes the key's header line. The one place a key goes into a request,
/// for every method.
fn push_auth(req: &mut String, auth: Option<(Auth, &ApiKey)>) {
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
}

/// Connects (through `proxy` for https) and exchanges `bytes`, the whole
/// request, within `timeout`.
async fn send(
    ep: &Endpoint,
    proxy: Option<&Proxy>,
    bytes: Vec<u8>,
    timeout: Duration,
) -> Result<Response, SendError> {
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
                    .map_err(|e| connect_error(e, format!("cannot reach {addr}")))?
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
    exchange(tls_connect(tcp, host).await?, req).await
}

/// The TLS session over `tcp`, its handshake done.
#[cfg(feature = "hosted")]
async fn tls_connect(
    tcp: TcpStream,
    host: &str,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, SendError> {
    let connector = tokio_rustls::TlsConnector::from(tls_config().map_err(SendError::Config)?);
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| SendError::Config(format!("`{host}` is not a TLS server name: {e}")))?;
    connector
        .connect(name, tcp)
        .await
        .map_err(|e| SendError::Broken(format!("TLS handshake with {host} failed: {e}")))
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

/// The bounds one streamed request runs under (#280).
///
/// Two, because neither alone ends a stream: colibri sends a keepalive
/// every 10 s for as long as it generates, so an idle bound never fires on a
/// stalled engine or a very long reply, and a deadline alone would let a
/// dead connection sit until it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamBounds {
    /// The longest the server may be silent. Any bytes from it, a
    /// keepalive comment included, start the bound again.
    pub idle: Duration,
    /// When the request ends, whatever is still arriving.
    pub deadline: Instant,
}

impl StreamBounds {
    /// `fut`, bounded by whichever comes first: `idle` from now, or the
    /// deadline. Each is its own error, so the caller can name the bound
    /// that ended the request.
    async fn within<T>(&self, fut: impl std::future::Future<Output = T>) -> Result<T, SendError> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(SendError::Deadline);
        }
        let wait = self.idle.min(left);
        tokio::time::timeout(wait, fut).await.map_err(|_| {
            if left <= self.idle {
                SendError::Deadline
            } else {
                SendError::Timeout
            }
        })
    }
}

/// How a streamed request ended.
#[derive(Debug)]
pub(crate) enum Streamed {
    /// A success status and an event stream that reached `[DONE]`; every
    /// event's data went to the caller's handler.
    Done {
        status: u16,
        headers: Vec<(String, String)>,
    },
    /// Any other status, read whole as [`post`] reads it, for the caller's
    /// retry policy. Nothing went to the handler.
    Status(Response),
}

/// Sends one POST asking for a server-sent event stream and hands each
/// event's `data` to `on_data` as it arrives, until `data: [DONE]`.
///
/// Never through a proxy: a streamed request goes only to this machine
/// (`HostedClient::with_streaming`), which `proxy_for` never proxies. A
/// stream that ends before `[DONE]` is an error, never a partial reply, and
/// every byte read counts against [`MAX_RESPONSE`], as a whole reply's do.
pub(crate) async fn post_stream<F>(
    ep: &Endpoint,
    path: &str,
    auth: Option<(Auth, &ApiKey)>,
    body: &str,
    bounds: StreamBounds,
    on_data: F,
) -> Result<Streamed, SendError>
where
    F: FnMut(&str) -> Result<(), String> + Send,
{
    let mut req = request_head(ep, path, body.len(), "text/event-stream");
    push_auth(&mut req, auth);
    req.push_str("\r\n");
    req.push_str(body);
    let addr = socket_addr(&ep.host, ep.port);
    let tcp = bounds
        .within(TcpStream::connect(&addr))
        .await?
        .map_err(|e| connect_error(e, format!("cannot reach {addr}")))?;
    if ep.tls {
        tls_stream(tcp, &ep.host, req.as_bytes(), bounds, on_data).await
    } else {
        stream_exchange(tcp, req.as_bytes(), bounds, on_data).await
    }
}

#[cfg(feature = "hosted")]
async fn tls_stream<F>(
    tcp: TcpStream,
    host: &str,
    req: &[u8],
    bounds: StreamBounds,
    on_data: F,
) -> Result<Streamed, SendError>
where
    F: FnMut(&str) -> Result<(), String> + Send,
{
    let s = bounds.within(tls_connect(tcp, host)).await??;
    stream_exchange(s, req, bounds, on_data).await
}

#[cfg(not(feature = "hosted"))]
async fn tls_stream<F>(
    _: TcpStream,
    _: &str,
    _: &[u8],
    _: StreamBounds,
    _: F,
) -> Result<Streamed, SendError>
where
    F: FnMut(&str) -> Result<(), String> + Send,
{
    Err(SendError::Config(NO_TLS.to_string()))
}

fn too_large() -> SendError {
    SendError::Broken(format!(
        "the response is larger than {} MiB, which no model reply is; nothing more was read",
        MAX_RESPONSE >> 20
    ))
}

/// The stream half of [`post_stream`]: every read under `bounds`, the
/// header parsed once, then the body decoded and split into events as it
/// arrives.
async fn stream_exchange<S, F>(
    mut s: S,
    req: &[u8],
    bounds: StreamBounds,
    mut on_data: F,
) -> Result<Streamed, SendError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnMut(&str) -> Result<(), String> + Send,
{
    let broken = |e: std::io::Error| SendError::Broken(format!("sending the request failed: {e}"));
    bounds.within(s.write_all(req)).await?.map_err(broken)?;
    bounds.within(s.flush()).await?.map_err(broken)?;
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    let mut read = 0usize;
    // One read, under the bounds and the cap. `Ok(0)` is the end of the
    // stream, a TLS close without close_notify included.
    macro_rules! next {
        () => {{
            match bounds.within(s.read(&mut buf)).await? {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => 0,
                Err(e) => {
                    return Err(SendError::Broken(format!(
                        "reading the response failed: {e}"
                    )));
                }
            }
        }};
    }
    let head_end = loop {
        let n = next!();
        if n == 0 {
            return Err(SendError::Broken(
                "the connection closed before the response header arrived".to_string(),
            ));
        }
        read += n;
        if read > MAX_RESPONSE {
            return Err(too_large());
        }
        let from = raw.len().saturating_sub(3);
        raw.extend_from_slice(&buf[..n]);
        if let Some(i) = find_subslice(&raw[from..], b"\r\n\r\n") {
            break from + i + 4;
        }
    };
    let head = parse_head(&raw[..head_end - 4]).map_err(SendError::Broken)?;
    if !(200..300).contains(&head.status) {
        // A refusal is a whole response, read as `exchange` reads one, so
        // the caller's retry policy sees exactly what it sees there.
        let mut framer = Framer::default();
        while !framer.complete(&raw) {
            if framer.declared().is_some_and(|n| n > MAX_RESPONSE) {
                return Err(too_large());
            }
            let n = next!();
            if n == 0 {
                break;
            }
            read += n;
            if read > MAX_RESPONSE {
                return Err(too_large());
            }
            raw.extend_from_slice(&buf[..n]);
        }
        return parse_response(&raw)
            .map(Streamed::Status)
            .map_err(SendError::Broken);
    }
    let mut body = BodyDecoder::new(&head.framing);
    let mut events = SseReader::default();
    let mut decoded = Vec::new();
    let mut feed = |bytes: &[u8], body: &mut BodyDecoder, events: &mut SseReader| {
        decoded.clear();
        body.feed(bytes, &mut decoded).map_err(SendError::Broken)?;
        events.feed(&decoded, &mut on_data)
    };
    feed(&raw[head_end..], &mut body, &mut events)?;
    drop(raw);
    while !events.done {
        if body.finished() {
            break;
        }
        let n = next!();
        if n == 0 {
            break;
        }
        read += n;
        if read > MAX_RESPONSE {
            return Err(too_large());
        }
        feed(&buf[..n], &mut body, &mut events)?;
    }
    if !events.done {
        return Err(SendError::Broken(
            "the event stream was cut short: it ended before `data: [DONE]`, so the reply \
             is incomplete and none of it was used"
                .to_string(),
        ));
    }
    Ok(Streamed::Done {
        status: head.status,
        headers: head.headers,
    })
}

/// A success response's body, undone from its transfer framing as it
/// arrives.
enum BodyDecoder {
    /// `Content-Length`: this many bytes still to come.
    Length(usize),
    Unframed,
    Chunked(Dechunker),
}

impl BodyDecoder {
    fn new(framing: &Framing) -> Self {
        match framing {
            Framing::Length(n) => Self::Length(*n),
            Framing::Unframed => Self::Unframed,
            Framing::Chunked => Self::Chunked(Dechunker::default()),
        }
    }

    fn feed(&mut self, b: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
        match self {
            Self::Length(left) => {
                let k = (*left).min(b.len());
                out.extend_from_slice(&b[..k]);
                *left -= k;
                Ok(())
            }
            Self::Unframed => {
                out.extend_from_slice(b);
                Ok(())
            }
            Self::Chunked(d) => d.feed(b, out),
        }
    }

    /// Whether the framing says the body is over.
    fn finished(&self) -> bool {
        match self {
            Self::Length(left) => *left == 0,
            Self::Unframed => false,
            Self::Chunked(d) => matches!(d.at, Dechunk::Done),
        }
    }
}

/// Chunked transfer coding undone a piece at a time: what [`dechunk`] does
/// to a whole body, for one that has not finished arriving.
#[derive(Default)]
struct Dechunker {
    /// A size or trailer line not yet ended.
    line: Vec<u8>,
    at: Dechunk,
}

#[derive(Default)]
enum Dechunk {
    #[default]
    Size,
    Data(usize),
    /// After a chunk's data: its CR, then its LF.
    DataCr,
    DataLf,
    Trailers,
    Done,
}

impl Dechunker {
    fn feed(&mut self, mut b: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
        while let Some(&first) = b.first() {
            match self.at {
                Dechunk::Size | Dechunk::Trailers => {
                    let Some(i) = b.iter().position(|&c| c == b'\n') else {
                        self.line.extend_from_slice(b);
                        if self.line.len() > MAX_SIZE_LINE {
                            return Err("a chunk size line is longer than any is".to_string());
                        }
                        return Ok(());
                    };
                    self.line.extend_from_slice(&b[..i]);
                    b = &b[i + 1..];
                    let line = std::mem::take(&mut self.line);
                    let line = line.strip_suffix(b"\r").unwrap_or(&line);
                    self.at = match self.at {
                        Dechunk::Size => match chunk_size(line)? {
                            0 => Dechunk::Trailers,
                            n => Dechunk::Data(n),
                        },
                        _ if line.is_empty() => Dechunk::Done,
                        _ => Dechunk::Trailers,
                    };
                }
                Dechunk::Data(n) => {
                    let k = n.min(b.len());
                    out.extend_from_slice(&b[..k]);
                    b = &b[k..];
                    self.at = if k == n {
                        Dechunk::DataCr
                    } else {
                        Dechunk::Data(n - k)
                    };
                }
                Dechunk::DataCr | Dechunk::DataLf => {
                    let want = if matches!(self.at, Dechunk::DataCr) {
                        b'\r'
                    } else {
                        b'\n'
                    };
                    if first != want {
                        return Err("a chunk's data does not end in CRLF".to_string());
                    }
                    b = &b[1..];
                    self.at = if want == b'\r' {
                        Dechunk::DataLf
                    } else {
                        Dechunk::Size
                    };
                }
                Dechunk::Done => return Ok(()),
            }
        }
        Ok(())
    }
}

/// Server-sent events, split from a body as it arrives (the WHATWG
/// event-stream format): lines end in LF or CRLF, a blank line ends an
/// event, `data:` lines are its data, and a line starting with `:` is a
/// comment, which is what a keepalive is.
#[derive(Default)]
struct SseReader {
    line: Vec<u8>,
    data: Option<String>,
    /// `data: [DONE]` arrived: the reply is whole.
    done: bool,
}

impl SseReader {
    fn feed<F>(&mut self, mut b: &[u8], on_data: &mut F) -> Result<(), SendError>
    where
        F: FnMut(&str) -> Result<(), String>,
    {
        while !self.done {
            let Some(i) = b.iter().position(|&c| c == b'\n') else {
                self.line.extend_from_slice(b);
                return Ok(());
            };
            self.line.extend_from_slice(&b[..i]);
            b = &b[i + 1..];
            let line = std::mem::take(&mut self.line);
            self.one_line(line.strip_suffix(b"\r").unwrap_or(&line), on_data)?;
        }
        Ok(())
    }

    fn one_line<F>(&mut self, line: &[u8], on_data: &mut F) -> Result<(), SendError>
    where
        F: FnMut(&str) -> Result<(), String>,
    {
        if line.is_empty() {
            match self.data.take() {
                Some(d) if d == "[DONE]" => self.done = true,
                Some(d) => on_data(&d).map_err(SendError::Stream)?,
                None => {}
            }
            return Ok(());
        }
        if line.starts_with(b":") {
            return Ok(());
        }
        let (field, value) = match line.iter().position(|&c| c == b':') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (line, &line[line.len()..]),
        };
        if field != b"data" {
            // `event`, `id` and `retry` say nothing a reply is made of.
            return Ok(());
        }
        let value = value.strip_prefix(b" ").unwrap_or(value);
        let value = std::str::from_utf8(value)
            .map_err(|_| SendError::Stream("an event's data is not UTF-8".to_string()))?;
        match &mut self.data {
            Some(d) => {
                d.push('\n');
                d.push_str(value);
            }
            None => self.data = Some(value.to_string()),
        }
        Ok(())
    }
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
            let Some(i) = find_subslice(&raw[from..], b"\r\n\r\n") else {
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
            Some(Body::Chunked { pos, at }) => advance_chunked(raw, pos, at, &mut self.examined),
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
fn advance_chunked(raw: &[u8], pos: &mut usize, at: &mut Chunk, examined: &mut usize) -> bool {
    loop {
        let rest = &raw[*pos..];
        match *at {
            Chunk::Size | Chunk::Trailers => {
                let window = &rest[..rest.len().min(MAX_SIZE_LINE + 2)];
                *examined += window.len();
                let Some(eol) = find_subslice(window, b"\r\n") else {
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

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub(crate) fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let end = find_subslice(raw, b"\r\n\r\n").ok_or("no HTTP header terminator in the response")?;
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
        let line_end = find_subslice(b, b"\r\n").ok_or("a chunk size line is unterminated")?;
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
            let head = request_head(&Endpoint::parse(base).unwrap(), "/x", 0, "application/json");
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

    #[test]
    fn a_chunked_body_whose_data_all_arrived_is_missing_its_next_size_line() {
        // Not "cut short inside a chunk": every byte of that chunk is here,
        // and #170 showed which of the two a user is told matters.
        let e = parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello")
            .unwrap_err();
        assert!(e.contains("size line is unterminated"), "{e}");
    }

    #[test]
    fn a_chunk_size_line_is_bounded_at_max_size_line_bytes() {
        // The risk: a size line that never ends held the read open, or a
        // legitimate one at the limit was refused.
        let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        let line = |len: usize| {
            let mut l = b"5;".to_vec();
            l.resize(len, b'a');
            l
        };
        let with = |parts: &[&[u8]]| {
            let mut raw = head.clone();
            parts.iter().for_each(|p| raw.extend_from_slice(p));
            raw
        };
        // At the limit and still unterminated: it may yet end.
        assert!(!complete(&with(&[&line(MAX_SIZE_LINE)])));
        // At the limit and terminated: a size line, and the chunk is awaited.
        assert!(!complete(&with(&[&line(MAX_SIZE_LINE), b"\r\nhel"])));
        // Past the limit: not a size line, whatever follows it.
        assert!(complete(&with(&[&line(MAX_SIZE_LINE + 904), b"\r\nhel"])));
    }

    #[test]
    fn bytes_after_a_chunk_that_are_not_crlf_end_the_read_at_once() {
        // Broken framing is handed to the parser now, not after the server
        // closes the connection or the request times out.
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhelloXY";
        assert!(complete(raw));
    }

    /// A stream that accepts any write and answers reads from `reads` in
    /// order, then end of stream. Stands in for a TLS session, whose read
    /// errors a loopback socket cannot be made to produce.
    struct Scripted(std::collections::VecDeque<std::io::Result<Vec<u8>>>);

    impl AsyncRead for Scripted {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(match self.0.pop_front() {
                Some(Ok(b)) => {
                    buf.put_slice(&b);
                    Ok(())
                }
                Some(Err(e)) => Err(e),
                None => Ok(()),
            })
        }
    }

    impl AsyncWrite for Scripted {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_close_without_close_notify_ends_the_read_and_no_other_error_does() {
        // A TLS peer that closes without close_notify reads as
        // UnexpectedEof: what arrived is judged by its framing. Any other
        // read error is a broken request, even after a body that parses.
        let reply = || Ok(b"HTTP/1.1 200 OK\r\n\r\n{}".to_vec());
        let s = Scripted([reply(), Err(std::io::ErrorKind::UnexpectedEof.into())].into());
        assert_eq!(exchange(s, b"x").await.unwrap().body, "{}");

        let s = Scripted([reply(), Err(std::io::ErrorKind::ConnectionReset.into())].into());
        match exchange(s, b"x").await {
            Err(SendError::Broken(e)) => assert!(e.contains("reading the response"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_response_exactly_at_the_cap_is_read_whole() {
        // The cap refuses one byte past it, on the header or on the bytes,
        // not a response that fills it.
        let head = "HTTP/1.1 200 OK\r\nContent-Length: ";
        // The length's own eight digits and the blank line count toward it.
        let body_len = MAX_RESPONSE - head.len() - 8 - 4;
        let mut raw = format!("{head}{body_len}\r\n\r\n").into_bytes();
        raw.resize(MAX_RESPONSE, b'x');
        let reads = raw.chunks(16 * 1024).map(|c| Ok(c.to_vec())).collect();
        let r = exchange(Scripted(reads), b"x").await.unwrap();
        assert_eq!(r.body.len(), body_len);
    }

    #[tokio::test]
    async fn post_sends_https_through_a_proxy_and_never_plain_http() {
        // The risk: plain http, key header included, sent to a proxy in the
        // clear; or https sent around the proxy the user configured.
        let remote = Endpoint::parse("https://api.openai.com/v1").unwrap();
        let (p, h) = connect_stub("403 Forbidden").await;
        match post(
            &remote,
            Some(&p),
            "/x",
            &[],
            None,
            "{}",
            Duration::from_secs(10),
        )
        .await
        {
            Err(SendError::Connect(e)) => assert!(e.contains("403"), "{e}"),
            other => panic!("the tunnel was not asked for: {other:?}"),
        }
        assert!(h.await.unwrap().starts_with("CONNECT api.openai.com:443"));

        let local = Endpoint::parse("http://127.0.0.1:9/v1").unwrap();
        let p = Proxy {
            host: "127.0.0.1".into(),
            port: 1,
        };
        match post(
            &local,
            Some(&p),
            "/x",
            &[],
            None,
            "{}",
            Duration::from_secs(10),
        )
        .await
        {
            Err(SendError::Config(e)) => assert!(e.contains("never sent through a proxy"), "{e}"),
            other => panic!("{other:?}"),
        }
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
        // Built at run time: a `user:pass@` literal reads as a leaked
        // basic-auth string to the secret scanner on every push (#185).
        // Leaked because `env` takes `'static` pairs; two short strings in a
        // test.
        let proxy =
            |pw: &str| -> &'static str { format!("http://alice:{pw}@proxy.example:3128").leak() };
        let e = proxy_for(&ep, &env(&[("HTTPS_PROXY", proxy("s3cret-pw"))]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("credentials"), "{e}");
        assert!(!e.contains("s3cret-pw") && !e.contains("alice"), "{e}");
        // A host NO_PROXY exempts never reads the proxy, so a proxy URL it
        // would refuse does not break it.
        assert_eq!(
            proxy_for(
                &ep,
                &env(&[("HTTPS_PROXY", proxy("pw")), ("NO_PROXY", "openai.com")])
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
    async fn a_connect_answer_with_a_few_kib_of_headers_opens_the_tunnel() {
        // The head bound is 16 KiB: a proxy that adds `Via` and diagnostic
        // headers is not refused for it.
        let status: &'static str =
            format!("200 Connection established\r\nX-Diag: {}", "d".repeat(2048)).leak();
        let (p, _h) = connect_stub(status).await;
        let target = Endpoint::parse("https://api.openai.com/v1").unwrap();
        connect_tunnel(&p, &target).await.expect("tunnel");
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

    #[test]
    fn connect_retry_only_an_os_connect_timeout_is_marked_retryable() {
        // The risk: the classification the retry rests on. A timeout is
        // kept apart; a refusal and a name that does not resolve are not.
        let kind = |k: std::io::ErrorKind| connect_error(std::io::Error::from(k), "to x".into());
        assert!(matches!(
            kind(std::io::ErrorKind::TimedOut),
            SendError::ConnectTimedOut(m) if m.starts_with("to x: ")
        ));
        for k in [
            std::io::ErrorKind::ConnectionRefused,
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::Other,
        ] {
            assert!(matches!(kind(k), SendError::Connect(_)), "{k:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn connect_retry_windows_connect_errors_map_to_the_kinds_the_retry_reads() {
        // #210's failure was `os error 10060`. If Rust stopped reading
        // WSAETIMEDOUT as `TimedOut`, the retry would silently stop applying.
        let os = |n: i32| std::io::Error::from_raw_os_error(n).kind();
        assert_eq!(os(10060), std::io::ErrorKind::TimedOut);
        assert_eq!(os(10061), std::io::ErrorKind::ConnectionRefused);
    }

    #[cfg(feature = "hosted")]
    #[test]
    fn the_tls_configuration_builds_with_the_compiled_in_roots() {
        // A provider or root-set mismatch would otherwise surface as a failed
        // handshake on a user's first hosted run.
        tls_config().expect("the ring provider and webpki roots configure");
    }
}
