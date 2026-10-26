//! The web view (ADR 0050 section 5): `trellis-desktop --web` serves the
//! window's page and its four commands over HTTP on 127.0.0.1, behind a
//! token drawn per launch, for `trellis --web`.
//!
//! What is served, all under the token:
//!
//! - `GET /?t=<token>`, the URL `trellis --web` opens: a redirect to
//!   `/<token>/`, the page's root. With the token in the path, the page's
//!   relative module imports and its fetches carry it on their own, so the
//!   page is served as it is, byte for byte, and nothing rewrites it;
//! - `GET /<token>/` and `GET /<token>/<file>`: the files under `ui/`,
//!   embedded at build time (`PAGE`);
//! - `POST /<token>/invoke/<command>`: the page's four commands, as
//!   `commands.rs` defines them: a JSON object of arguments in, `{"ok":
//!   <value>}` or `{"error": "<why>"}` out, the same values and refusals
//!   the window's IPC carries;
//! - `GET /<token>/events`: the core's events, one server-sent `event:` per
//!   `Sink::emit`, to every page that is open. A page that falls behind
//!   loses its oldest unsent events rather than holding the core's memory
//!   (`STREAM_BACKLOG` and `STREAM_BYTES`, #814).
//!
//! A request whose path does not carry the token, or whose Host header is
//! not `127.0.0.1:<port>` or `localhost:<port>`, is refused with 403 once
//! its request line and headers are read and before anything else is: not
//! its body, not a file, not the core. The Host rule is the DNS-rebinding
//! guard: a page on another origin can make a name resolve to 127.0.0.1,
//! but the request it sends then carries that name as its Host. Only
//! 127.0.0.1 is bound, on a free port. The token is 32 bytes from the OS's
//! random source, written once on stdout inside a JSON object for `trellis
//! --web` to read, and nowhere else.
//!
//! Every answer, a refusal too, ends with the write side shut and what the
//! client sent that was not read thrown away unparsed, within a short bound
//! (`linger`), so the close is a FIN and not a reset, which on Windows
//! loses the answer. Throwing bytes away is not reading them for what they
//! say: a refusal is still decided on the request line and headers alone.
//!
//! The page is the window's page. `ui/web.js`, loaded first and the one
//! module that fetches, defines `window.__TAURI__` over `fetch` and
//! `EventSource` when Tauri has not, so every other module runs unchanged
//! (ADR 0048 section 5): the page reaches the core through the same four
//! commands and the same events as the window, and there is no path around
//! the orchestrator (section 10). Every string still reaches the DOM
//! through `textContent`, and the response headers carry a policy as
//! strict as the window's (`csp_header`).
//!
//! The server stops when its stdin ends. `trellis --web` closes it on
//! Ctrl-C, and the core then sends the rpc child `quit` and waits for it, as
//! the window does when it closes (ADR 0048 section 9): nothing is killed.

use crate::capped::Capped;
use crate::child::LINE_READ_CAP;
use crate::commands::{Core, Sink};
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

/// The page's files, by name, as `ui/` holds them. A file added to `ui/`
/// is added here, or `web_serves_the_files_under_ui_and_nothing_else`
/// fails.
pub const PAGE: &[(&str, &[u8])] = &[
    ("index.html", include_bytes!("../ui/index.html")),
    ("ds.css", include_bytes!("../ui/ds.css")),
    ("style.css", include_bytes!("../ui/style.css")),
    ("web.js", include_bytes!("../ui/web.js")),
    ("main.js", include_bytes!("../ui/main.js")),
    ("render.js", include_bytes!("../ui/render.js")),
    ("overview.js", include_bytes!("../ui/overview.js")),
    ("composer.js", include_bytes!("../ui/composer.js")),
    ("dialogs.js", include_bytes!("../ui/dialogs.js")),
    ("model.js", include_bytes!("../ui/model.js")),
    ("settings.js", include_bytes!("../ui/settings.js")),
    ("status.js", include_bytes!("../ui/status.js")),
    ("graph.js", include_bytes!("../ui/graph.js")),
    ("now.js", include_bytes!("../ui/now.js")),
    ("codebase.js", include_bytes!("../ui/codebase.js")),
    ("radius.js", include_bytes!("../ui/radius.js")),
    ("trails.js", include_bytes!("../ui/trails.js")),
    ("vault.js", include_bytes!("../ui/vault.js")),
    ("queue.js", include_bytes!("../ui/queue.js")),
    ("atlas.js", include_bytes!("../ui/atlas.js")),
    ("dom.js", include_bytes!("../ui/dom.js")),
    ("fmt.js", include_bytes!("../ui/fmt.js")),
    ("rpc.js", include_bytes!("../ui/rpc.js")),
    ("ds.js", include_bytes!("../ui/ds.js")),
    ("favicon.svg", include_bytes!("../ui/favicon.svg")),
    ("keys.js", include_bytes!("../ui/keys.js")),
    ("code.js", include_bytes!("../ui/code.js")),
    ("summary.js", include_bytes!("../ui/summary.js")),
    ("plan.js", include_bytes!("../ui/plan.js")),
];

/// The page's commands (`commands.rs`), the same four `build.rs` lists.
pub const COMMANDS: [&str; 4] = ["rpc_send", "get_trails", "open_repository", "get_overview"];

/// The longest request line and header block read, in bytes.
const HEAD_MAX: usize = 16 * 1024;
/// The longest request body read: above `rpc_send`'s own line cap with
/// room for the JSON around it.
const BODY_MAX: usize = 1024 * 1024;
/// How long a connection may take to send its request line and headers.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How long, in all, `linger` reads and throws away what a client sent
/// past what its answer needed, and at most how many bytes of it.
const LINGER: Duration = Duration::from_millis(200);
const LINGER_MAX: usize = 64 * 1024;
/// How often an idle event stream carries a comment, so a page that went
/// away is noticed by the write that fails.
const KEEPALIVE: Duration = Duration::from_secs(15);
/// Events an event stream may fall behind by, by count and by bytes, before
/// its oldest unsent ones are dropped: a page that stopped reading is not
/// let hold the core's memory (#814), and the core never waits on it.
const STREAM_BACKLOG: usize = 1024;
/// Twice the longest line `child.rs` reads, so the `rpc-frame` of any line
/// read whole fits a stream with room beside it.
const STREAM_BYTES: usize = 2 * LINE_READ_CAP;

/// What the response to the page carries, as strict as the window's policy
/// (`tauri.conf.json`, held by `lockdown.rs`): nothing inline, nothing from
/// another origin, and `connect-src 'self'` for the page's own fetches and
/// its event stream.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
                       connect-src 'self'; object-src 'none'; base-uri 'none'; \
                       form-action 'none'; frame-ancestors 'none'";

/// One page's event stream: the frames the core emitted that the page has
/// not been sent yet, held to [`STREAM_BACKLOG`] and [`STREAM_BYTES`].
#[derive(Default)]
struct Stream {
    unsent: Mutex<Capped<STREAM_BACKLOG, STREAM_BYTES>>,
    /// Woken by each push, so the stream's thread sends without polling.
    pushed: Condvar,
}

impl Stream {
    fn push(&self, frame: String) {
        self.unsent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(frame);
        self.pushed.notify_one();
    }

    /// The oldest unsent frame, waiting up to `wait` for one. It is taken
    /// out under the lock and written after it, so a page slow to read
    /// never holds up a push.
    fn next(&self, wait: Duration) -> Option<String> {
        let unsent = self.unsent.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut unsent, _) = self
            .pushed
            .wait_timeout_while(unsent, wait, |u| u.is_empty())
            .unwrap_or_else(PoisonError::into_inner);
        unsent.pop_front()
    }
}

/// The core's events, fanned out to every open event stream.
#[derive(Default)]
pub struct WebSink {
    /// Held weakly: a stream's own connection thread holds it, so a stream
    /// whose page went away is gone, and dropped here at the next emit.
    streams: Mutex<Vec<Weak<Stream>>>,
}

impl WebSink {
    /// A new stream: what the core emits from now on.
    fn subscribe(&self) -> Arc<Stream> {
        let stream = Arc::new(Stream::default());
        self.streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::downgrade(&stream));
        stream
    }

    /// How many streams are open.
    #[cfg(test)]
    pub fn open(&self) -> usize {
        self.streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|s| s.strong_count() > 0)
            .count()
    }
}

impl Sink for WebSink {
    fn emit(&self, event: &str, payload: Value) {
        // One JSON object on one line: serde_json escapes the newlines in a
        // string, and the event names are the core's own constants.
        let frame = format!("event: {event}\ndata: {payload}\n\n");
        // Never cut: half an event is not one the page can read, so one past
        // a stream's whole byte cap reaches no page, and that is said.
        if frame.len() > STREAM_BYTES {
            eprintln!(
                "trellis-desktop: web: a {event} event of {} bytes is past the {STREAM_BYTES}-byte \
                 stream cap, so no page was sent it",
                frame.len()
            );
            return;
        }
        // A stream whose page went away is dropped here; one that fell
        // behind loses its oldest unsent frames. Neither is waited on.
        self.streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|s| match s.upgrade() {
                Some(stream) => {
                    stream.push(frame.clone());
                    true
                }
                None => false,
            });
    }
}

/// 32 bytes from the OS's random source, as 64 hex characters.
///
/// std has no random API, and a crate for one is a dependency ADR 0048
/// section 7 would have to admit, so the bytes come from the source std
/// keys its hash maps from: a `RandomState` made on a fresh thread holds
/// 128 bits the OS drew for that thread and nothing else has used, and
/// SipHash under those keys over the counters 0 to 3 is a keyed function
/// whose outputs cannot be guessed without them.
pub fn token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let bytes = std::thread::spawn(|| {
        let keys = RandomState::new();
        let mut out = [0u8; 32];
        for (i, chunk) in out.chunks_mut(8).enumerate() {
            let mut h = keys.build_hasher();
            h.write_u64(i as u64);
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
        out
    })
    .join()
    .expect("the token thread does not panic");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Binds the one address the web view listens on: 127.0.0.1, a free port.
pub fn bind() -> io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
}

/// The URL `trellis --web` opens for a listener on `port`.
pub fn url(port: u16, token: &str) -> String {
    format!("http://127.0.0.1:{port}/?t={token}")
}

/// What runs when the first page connects its event stream: the window's
/// `on_page_load`, which opens the startup repository (`Core::startup`).
/// Once, so a reload starts nothing twice.
type Startup = Box<dyn FnOnce() + Send>;

/// One web view: the listener, its token, the core and what starts it.
pub struct Server {
    listener: TcpListener,
    token: String,
    core: Core,
    sink: Arc<WebSink>,
    startup: Mutex<Option<Startup>>,
    cwd: PathBuf,
}

impl Server {
    /// A server on `listener` for `core`, whose events reach the pages
    /// through `sink`; `startup` runs on the first event stream.
    pub fn new(
        listener: TcpListener,
        core: Core,
        sink: Arc<WebSink>,
        startup: Startup,
        cwd: PathBuf,
    ) -> Self {
        Self {
            listener,
            token: token(),
            core,
            sink,
            startup: Mutex::new(Some(startup)),
            cwd,
        }
    }

    pub fn port(&self) -> u16 {
        self.listener.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    #[cfg(test)]
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn url(&self) -> String {
        url(self.port(), &self.token)
    }

    /// Accepts connections until the listener fails, one thread each.
    pub fn run(self: Arc<Self>) -> io::Error {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let server = Arc::clone(&self);
                    std::thread::spawn(move || {
                        // A page that went away mid-reply, or closed its
                        // event stream, is the usual way a connection
                        // ends; anything else is said on stderr, never to
                        // a page.
                        if let Err(e) = server.handle(stream)
                            && !matches!(
                                e.kind(),
                                io::ErrorKind::BrokenPipe
                                    | io::ErrorKind::ConnectionReset
                                    | io::ErrorKind::ConnectionAborted
                            )
                        {
                            eprintln!("trellis-desktop: web: {e}");
                        }
                    });
                }
                Err(e) => return e,
            }
        }
    }

    /// One connection: its answer, then its close.
    fn handle(&self, stream: TcpStream) -> io::Result<()> {
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut out = stream;
        self.answer(&mut reader, &mut out)?;
        linger(&mut out)
    }

    /// The request line and headers, the two guards, then the route.
    fn answer(&self, reader: &mut BufReader<TcpStream>, out: &mut TcpStream) -> io::Result<()> {
        let request = match read_head(reader) {
            Ok(r) => r,
            Err(status) => {
                return respond(
                    out,
                    status,
                    "text/plain",
                    status_text(status).as_bytes(),
                    &[],
                );
            }
        };
        // Both guards before anything else is read or looked up, and one
        // answer for every way they fail: which one failed is not told.
        if !self.host_allowed(request.header("host")) {
            return respond(out, 403, "text/plain", b"forbidden\n", &[]);
        }
        let (path, query) = request
            .target
            .split_once('?')
            .unwrap_or((&request.target, ""));
        if path == "/" && query_token(query).is_some_and(|t| same(t, &self.token)) {
            let location = format!("/{}/", self.token);
            return respond(
                out,
                303,
                "text/plain",
                b"see the page\n",
                &[("Location", &location)],
            );
        }
        let Some(rest) = path
            .strip_prefix('/')
            .and_then(|p| p.split_once('/'))
            .filter(|(t, _)| same(t, &self.token))
            .map(|(_, rest)| rest)
        else {
            return respond(out, 403, "text/plain", b"forbidden\n", &[]);
        };
        match (request.method.as_str(), rest) {
            ("GET", "events") => self.events(out),
            ("GET", "") => page(out, "index.html"),
            ("GET", name) if name.starts_with("invoke/") => {
                respond(out, 405, "text/plain", b"a command is a POST\n", &[])
            }
            ("GET", name) => page(out, name),
            ("POST", name) if name.starts_with("invoke/") => {
                let command = &name["invoke/".len()..];
                let body = match read_body(reader, &request) {
                    Ok(b) => b,
                    Err(status) => {
                        return respond(
                            out,
                            status,
                            "text/plain",
                            status_text(status).as_bytes(),
                            &[],
                        );
                    }
                };
                let (status, answer) = self.invoke(command, &body);
                respond(
                    out,
                    status,
                    "application/json",
                    answer.to_string().as_bytes(),
                    &[],
                )
            }
            _ => respond(out, 405, "text/plain", b"method not allowed\n", &[]),
        }
    }

    /// The Host guard: this port on 127.0.0.1 or localhost, nothing else.
    fn host_allowed(&self, host: Option<&str>) -> bool {
        let port = self.port();
        host.is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"))
    }

    /// The event stream: subscribed before startup runs, so the first
    /// page hears what startup emits, then every frame until the page goes.
    fn events(&self, out: &mut TcpStream) -> io::Result<()> {
        let stream = self.sink.subscribe();
        write_head(out, 200, "text/event-stream", None, &[])?;
        out.write_all(b": connected\n\n")?;
        out.flush()?;
        if let Some(start) = self
            .startup
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            std::thread::spawn(start);
        }
        // Until a write fails: the page went away, and its stream with this
        // thread.
        loop {
            match stream.next(KEEPALIVE) {
                Some(frame) => out.write_all(frame.as_bytes())?,
                None => out.write_all(b": keepalive\n\n")?,
            }
            out.flush()?;
        }
    }

    /// One of the page's four commands, over the same core methods the
    /// window's commands call, with the same refusals.
    fn invoke(&self, command: &str, body: &[u8]) -> (u16, Value) {
        if !COMMANDS.contains(&command) {
            return (
                404,
                json!({"error": format!("no such command: {}", printable(command))}),
            );
        }
        let args: Value = match serde_json::from_slice(body) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => return (400, json!({"error": "the arguments are a JSON object"})),
            Err(e) => {
                return (
                    400,
                    json!({"error": format!("the arguments are not JSON: {e}")}),
                );
            }
        };
        let result: Result<Value, String> = match command {
            "rpc_send" => match args.get("line") {
                Some(line) => self.core.send(line).map(|()| Value::Null),
                None => Err("rpc_send: no `line`".into()),
            },
            "get_trails" => match args.get("limit").and_then(Value::as_u64) {
                Some(n) => u32::try_from(n)
                    .map_err(|_| format!("get_trails: limit must be 1 to 200; got {n}"))
                    .and_then(|n| self.core.trails(n))
                    .map(Value::Array),
                None => Err("get_trails: no `limit`".into()),
            },
            "open_repository" => match args.get("path").and_then(Value::as_str) {
                Some(p) => self.core.open(Path::new(p)).map(|()| Value::Null),
                None => Err("open_repository: no `path`".into()),
            },
            _ => self.core.overview(&self.cwd),
        };
        match result {
            Ok(v) => (200, json!({"ok": v})),
            Err(e) => (400, json!({"error": e})),
        }
    }
}

/// Serves `core` for `trellis --web`: prints the URL as one JSON line on
/// stdout, stops when stdin ends, and otherwise runs until the listener
/// fails. `root` is the startup repository, as `--root` gave it.
pub fn serve(core: Core, sink: Arc<WebSink>, root: Option<PathBuf>) -> io::Error {
    let listener = match bind() {
        Ok(l) => l,
        Err(e) => return e,
    };
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => return e,
    };
    let startup_core = core.clone();
    let startup_cwd = cwd.clone();
    let startup: Startup = Box::new(move || startup_core.startup(root, &startup_cwd));
    let server = Arc::new(Server::new(listener, core.clone(), sink, startup, cwd));
    // The one line stdout carries: `trellis --web` reads it and opens the
    // URL; nothing else is written there.
    let mut stdout = io::stdout().lock();
    if let Err(e) =
        writeln!(stdout, "{}", json!({"url": server.url()})).and_then(|()| stdout.flush())
    {
        return e;
    }
    drop(stdout);
    // Stdin ending is the request to stop (ADR 0048 section 9 for the child).
    std::thread::spawn(move || {
        let mut sink = [0u8; 1024];
        let mut stdin = io::stdin().lock();
        while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
        if let Some(taken) = core.take_child("the web view is closing")
            && let Err(e) = core.shut_down(taken)
        {
            eprintln!("trellis-desktop: shutting down trellis rpc: {e}");
        }
        std::process::exit(0);
    });
    server.run()
}

/// A request's line and headers.
struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Reads the request line and headers, up to `HEAD_MAX` bytes. `Err` is
/// the status to answer with: 400 for a line that is not HTTP, 431 for a
/// head past the cap, 408 for one that did not arrive in time.
fn read_head(reader: &mut BufReader<TcpStream>) -> Result<Request, u16> {
    let mut read = 0usize;
    let mut line = String::new();
    let mut next = |line: &mut String| -> Result<(), u16> {
        line.clear();
        match reader.read_line(line) {
            Ok(0) => Err(400),
            Ok(n) => {
                read += n;
                if read > HEAD_MAX {
                    return Err(431);
                }
                Ok(())
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Err(408)
            }
            Err(_) => Err(400),
        }
    };
    next(&mut line)?;
    let mut parts = line.trim_end_matches(['\r', '\n']).splitn(3, ' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(400);
    };
    if !version.starts_with("HTTP/1.") || target.is_empty() || !target.starts_with('/') {
        return Err(400);
    }
    let mut request = Request {
        method: method.to_string(),
        target: target.to_string(),
        headers: Vec::new(),
    };
    loop {
        next(&mut line)?;
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            return Ok(request);
        }
        let Some((name, value)) = l.split_once(':') else {
            return Err(400);
        };
        request
            .headers
            .push((name.trim().to_string(), value.trim().to_string()));
    }
}

/// Reads a POST's body: `Content-Length` required, at most `BODY_MAX`.
fn read_body(reader: &mut BufReader<TcpStream>, request: &Request) -> Result<Vec<u8>, u16> {
    let len: usize = request
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .ok_or(411u16)?;
    if len > BODY_MAX {
        return Err(413u16);
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).map_err(|e| -> u16 {
        if matches!(
            e.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ) {
            408
        } else {
            400
        }
    })?;
    Ok(body)
}

/// The `t` parameter of a query string, undecoded: the token is hex.
fn query_token(query: &str) -> Option<&str> {
    query.split('&').find_map(|p| p.strip_prefix("t="))
}

/// Whether two tokens are equal, read in full whatever the first
/// difference, so the comparison's time says nothing about the token.
fn same(given: &str, token: &str) -> bool {
    let (a, b) = (given.as_bytes(), token.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The file `name` under `ui/`, or 404. A name is one path segment of
/// ASCII letters, digits, `.`, `_` and `-`, so nothing is ever looked up
/// outside `PAGE`.
fn page(out: &mut TcpStream, name: &str) -> io::Result<()> {
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
    match plain
        .then(|| PAGE.iter().find(|(n, _)| *n == name))
        .flatten()
    {
        Some((n, bytes)) => respond(out, 200, mime(n), bytes, &[]),
        None => respond(out, 404, "text/plain", b"no such file\n", &[]),
    }
}

/// The media type of a page file, by its extension.
fn mime(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn status_text(status: u16) -> String {
    let reason = match status {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    format!("{status} {reason}\n")
}

/// The status line and the headers every response carries: no caching, no
/// sniffing, no referrer, the policy, and `Connection: close`, since each
/// request is its own connection. `length` is `None` for a stream.
fn write_head(
    out: &mut TcpStream,
    status: u16,
    content_type: &str,
    length: Option<usize>,
    extra: &[(&str, &str)],
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {}", status_text(status).trim_end());
    head.push_str("\r\n");
    head.push_str(&format!("Content-Type: {content_type}\r\n"));
    if let Some(n) = length {
        head.push_str(&format!("Content-Length: {n}\r\n"));
    }
    head.push_str(&format!("Content-Security-Policy: {CSP}\r\n"));
    head.push_str("Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n");
    head.push_str("Referrer-Policy: no-referrer\r\nConnection: close\r\n");
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())
}

fn respond(
    out: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) -> io::Result<()> {
    write_head(out, status, content_type, Some(body.len()), extra)?;
    out.write_all(body)?;
    out.flush()
}

/// Ends a connection whose answer is written: shuts the write side, so the
/// answer is followed by a FIN, then reads and throws away what the client
/// sent that was never read, until it closes, `LINGER` has passed or
/// `LINGER_MAX` bytes have come; only then is the socket dropped.
///
/// A socket closed with input still unread sends a reset instead of a FIN,
/// and on Windows a reset fails the client's next read and discards the
/// answer it had received but not yet read: a request refused once its
/// head is read, with a body behind it, would see a dropped connection
/// where its 403 should be. The shutdown also sends the FIN while another
/// handle to the socket is still open, where dropping this one alone would
/// not. Nothing drained here is parsed, kept or acted on, so a refusal
/// still comes before anything past the head is read for what it says.
/// Both bounds hold over the whole drain, not per read, so a client that
/// trickles bytes holds the thread for `LINGER` at most.
fn linger(out: &mut TcpStream) -> io::Result<()> {
    match out.shutdown(Shutdown::Write) {
        Ok(()) => {}
        // The client reset first: no one is left to read a FIN.
        Err(e) if e.kind() == io::ErrorKind::NotConnected => return Ok(()),
        Err(e) => return Err(e),
    }
    let deadline = Instant::now() + LINGER;
    let mut buf = [0u8; 8192];
    let mut drained = 0usize;
    while drained < LINGER_MAX {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        out.set_read_timeout(Some(left))?;
        let room = buf.len().min(LINGER_MAX - drained);
        match out.read(&mut buf[..room]) {
            Ok(0) => break,
            Ok(n) => drained += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// A request-supplied name as it is written back: ASCII printable only,
/// cut short, so an answer cannot carry a control sequence.
fn printable(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_graphic())
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::child::Trellis;
    use std::collections::BTreeSet;

    /// A core over a trellis that is never started: no test here reaches a
    /// child, and every refusal they check comes before one would.
    fn server(startup: Startup) -> (Arc<Server>, Arc<WebSink>) {
        let sink = Arc::new(WebSink::default());
        let core = Core::new(
            Trellis::new(PathBuf::from("trellis-not-started-by-this-test")),
            sink.clone(),
        );
        let server = Arc::new(Server::new(
            bind().unwrap(),
            core,
            sink.clone(),
            startup,
            std::env::temp_dir(),
        ));
        let running = Arc::clone(&server);
        std::thread::spawn(move || running.run());
        (server, sink)
    }

    fn quiet() -> (Arc<Server>, Arc<WebSink>) {
        server(Box::new(|| {}))
    }

    /// A response's status, its headers with their names lowercased, and
    /// its body.
    type Response = (u16, Vec<(String, String)>, Vec<u8>);

    /// A connection with `raw` written on it.
    fn send(server: &Server, raw: &[u8]) -> TcpStream {
        let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        s.write_all(raw).unwrap();
        s
    }

    /// One raw request; the whole response once the server closes.
    ///
    /// A reset is taken as the close only once the head and exactly the
    /// body its Content-Length names are in hand, so a reset that came
    /// where the response should be, or cut it short, fails here and is
    /// never read as a refusal. That the close is a FIN and not a reset is
    /// held apart, by
    /// `web_a_refused_request_still_reads_its_403_when_the_client_sent_more`,
    /// which takes no reset at all.
    fn request(server: &Server, raw: &str) -> Response {
        let mut s = send(server, raw.as_bytes());
        let mut all = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => all.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                    ) && parse(&all).is_some() =>
                {
                    break;
                }
                Err(e) => panic!(
                    "{e} after {} bytes: {:?}",
                    all.len(),
                    String::from_utf8_lossy(&all)
                ),
            }
        }
        parse(&all)
            .unwrap_or_else(|| panic!("not a whole response: {:?}", String::from_utf8_lossy(&all)))
    }

    /// A response's status, headers and body; `None` until the head and as
    /// many body bytes as its Content-Length names, and no more, are there.
    fn parse(all: &[u8]) -> Option<Response> {
        let split = all.windows(4).position(|w| w == b"\r\n\r\n")?;
        let head = String::from_utf8(all[..split].to_vec()).unwrap();
        let body = all[split + 4..].to_vec();
        let mut lines = head.lines();
        let status: u16 = lines
            .next()
            .unwrap()
            .split(' ')
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers: Vec<(String, String)> = lines
            .map(|l| {
                let (n, v) = l.split_once(':').unwrap();
                (n.trim().to_ascii_lowercase(), v.trim().to_string())
            })
            .collect();
        let length: usize = header(&headers, "content-length")
            .expect("every answer but the event stream names its length")
            .parse()
            .unwrap();
        (body.len() == length).then_some((status, headers, body))
    }

    fn get(server: &Server, target: &str, host: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
        request(
            server,
            &format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        )
    }

    fn post(server: &Server, target: &str, body: &str) -> (u16, Value) {
        let (status, _, answer) = request(
            server,
            &format!(
                "POST {target} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                server.port(),
                body.len()
            ),
        );
        (status, serde_json::from_slice(&answer).unwrap())
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn host(server: &Server) -> String {
        format!("127.0.0.1:{}", server.port())
    }

    #[test]
    fn web_listener_binds_only_127_0_0_1_on_a_free_port() {
        // The risk (ADR 0050 section 5): the page, and with it the core's
        // commands, reachable from another machine.
        let l = bind().unwrap();
        let addr = l.local_addr().unwrap();
        assert_eq!(addr.ip(), std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(addr.port(), 0);
        let (server, _) = quiet();
        let addr = server.listener.local_addr().unwrap();
        assert_eq!(addr.ip(), std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(
            server.url(),
            format!("http://127.0.0.1:{}/?t={}", addr.port(), server.token())
        );
    }

    #[test]
    fn web_token_is_32_bytes_drawn_per_launch() {
        // The risk: a token short enough to guess, or one that repeats
        // between launches, so a page that learned it once keeps it.
        let a = token();
        let b = token();
        assert_eq!(a.len(), 64, "{a}");
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()), "{a}");
        assert_ne!(a, b);
        assert_ne!(
            &a[..32],
            &a[32..],
            "the two halves are not one value repeated"
        );
        assert!(same(&a, &a.clone()) && !same(&a, &b) && !same(&a[..63], &a));
    }

    #[test]
    fn web_a_request_without_the_token_gets_403_before_its_body_is_read() {
        // The risk: the page, a file or a command served to a request that
        // did not come from the URL this launch printed.
        let (server, _) = quiet();
        let h = host(&server);
        for target in [
            "/",
            "/?t=",
            "/?t=0000",
            "/index.html",
            "/main.js",
            "/events",
            "/invoke/get_overview",
        ] {
            let (status, _, body) = get(&server, target, &h);
            assert_eq!(status, 403, "{target}");
            assert_eq!(body, b"forbidden\n", "{target}: the refusal names nothing");
        }
        let wrong = format!("/{}/", "0".repeat(64));
        assert_eq!(
            get(&server, &wrong, &h).0,
            403,
            "a token of the right shape"
        );
        let short = format!("/{}/", &server.token()[..63]);
        assert_eq!(get(&server, &short, &h).0, 403, "a prefix of the token");
        // A POST whose body never arrives is still answered: the body is
        // not waited for before the refusal.
        let started = std::time::Instant::now();
        let (status, _, _) = request(
            &server,
            &format!("POST /invoke/rpc_send HTTP/1.1\r\nHost: {h}\r\nContent-Length: 100\r\n\r\n"),
        );
        assert_eq!(status, 403);
        assert!(
            started.elapsed() < READ_TIMEOUT,
            "answered before the read timeout"
        );
    }

    #[test]
    fn web_a_request_with_a_foreign_host_header_gets_403() {
        // The risk (ADR 0050 section 5): DNS rebinding, a name another
        // origin pointed at 127.0.0.1, reaching the page with the token.
        let (server, _) = quiet();
        let port = server.port();
        let root = format!("/{}/", server.token());
        for h in [
            "evil.example".to_string(),
            format!("evil.example:{port}"),
            format!("127.0.0.1:{}", port.wrapping_add(1)),
            "127.0.0.1".to_string(),
            format!("127.0.0.2:{port}"),
            format!("[::1]:{port}"),
            format!("localhost.:{port}"),
        ] {
            let (status, _, body) = get(&server, &root, &h);
            assert_eq!(status, 403, "Host: {h}");
            assert_eq!(body, b"forbidden\n");
        }
        let (status, _, _) = request(&server, &format!("GET {root} HTTP/1.1\r\n\r\n"));
        assert_eq!(status, 403, "no Host header");
    }

    #[test]
    fn web_a_refused_request_still_reads_its_403_when_the_client_sent_more() {
        // The risk (#666's Windows CI): a request refused once its head is
        // read, with its body still unread, closed with a reset instead of
        // a FIN. On Windows the reset makes the client's next read fail
        // and throws away the 403 it had received, so the page sees a
        // dropped connection where the refusal should be. Linux hands the
        // 403 over and then fails the read, so any reset fails this test:
        // the client reads to the server's close and takes no error. The
        // body is past the server's first read of 8 KiB, so most of it is
        // still in the socket when the refusal is written.
        let (server, _) = quiet();
        let body = "x".repeat(32 * 1024);
        let root = format!("/{}/", server.token());
        for (target, h) in [
            (format!("{root}invoke/rpc_send"), "evil.example".to_string()),
            ("/invoke/rpc_send".to_string(), host(&server)),
        ] {
            let raw = format!(
                "POST {target} HTTP/1.1\r\nHost: {h}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let mut s = send(&server, raw.as_bytes());
            let mut all = Vec::new();
            if let Err(e) = s.read_to_end(&mut all) {
                panic!(
                    "Host: {h}, {target}: the answer ended in {e:?}, not a close, after {:?}",
                    String::from_utf8_lossy(&all)
                );
            }
            let (status, _, got) = parse(&all).expect("a whole response");
            assert_eq!(status, 403, "Host: {h}, {target}");
            assert_eq!(got, b"forbidden\n", "Host: {h}, {target}");
        }
    }

    #[test]
    fn web_the_token_with_the_right_host_gets_the_page() {
        // The ticket's own case (#375): the URL trellis --web opens leads
        // to the window's page, byte for byte, on either name for this
        // machine.
        let (server, _) = quiet();
        let h = host(&server);
        let (status, headers, _) = get(&server, &format!("/?t={}", server.token()), &h);
        assert_eq!(status, 303);
        let root = format!("/{}/", server.token());
        assert_eq!(header(&headers, "location"), Some(root.as_str()));
        for name in [h.clone(), format!("localhost:{}", server.port())] {
            let (status, headers, body) = get(&server, &root, &name);
            assert_eq!(status, 200, "Host: {name}");
            assert_eq!(
                header(&headers, "content-type"),
                Some("text/html; charset=utf-8")
            );
            assert_eq!(body, std::fs::read(ui("index.html")).unwrap());
        }
        let (status, headers, body) = get(&server, &format!("{root}main.js"), &h);
        assert_eq!(status, 200);
        assert_eq!(
            header(&headers, "content-type"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(body, std::fs::read(ui("main.js")).unwrap());
        let (status, headers, _) = get(&server, &format!("{root}style.css"), &h);
        assert_eq!(
            (status, header(&headers, "content-type")),
            (200, Some("text/css; charset=utf-8"))
        );
    }

    #[test]
    fn web_every_response_carries_the_window_s_policy() {
        // The risk: the page served over http with a looser policy than the
        // window's (`lockdown.rs`): an inline script, a network origin, or
        // the page framed by another origin.
        let (server, _) = quiet();
        let root = format!("/{}/", server.token());
        for target in [root.clone(), format!("{root}main.js"), "/".to_string()] {
            let (_, headers, _) = get(&server, &target, &host(&server));
            let csp = header(&headers, "content-security-policy").expect("a policy");
            assert!(!csp.contains("unsafe"), "{target}: {csp}");
            assert!(
                !csp.contains("http:") && !csp.contains("https:") && !csp.contains("ws"),
                "{target}: {csp}"
            );
            for want in [
                "default-src 'none'",
                "script-src 'self'",
                "connect-src 'self'",
                "frame-ancestors 'none'",
                "base-uri 'none'",
                "object-src 'none'",
            ] {
                assert!(csp.contains(want), "{target}: {csp} lacks {want}");
            }
            assert_eq!(header(&headers, "cache-control"), Some("no-store"));
            assert_eq!(header(&headers, "x-content-type-options"), Some("nosniff"));
            assert_eq!(header(&headers, "referrer-policy"), Some("no-referrer"));
        }
    }

    #[test]
    fn web_serves_the_files_under_ui_and_nothing_else() {
        // The risk: a file added to ui/ that the web view does not serve,
        // so the page breaks only there; or a path that reaches past PAGE.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let on_disk: BTreeSet<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        let served: BTreeSet<String> = PAGE.iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(served, on_disk);
        for (name, bytes) in PAGE {
            assert_eq!(
                *bytes,
                std::fs::read(dir.join(name)).unwrap().as_slice(),
                "{name}"
            );
        }
        let (server, _) = quiet();
        let root = format!("/{}/", server.token());
        for bad in [
            "../Cargo.toml",
            "..%2FCargo.toml",
            "nope.js",
            ".git",
            "src/main.rs",
            "index.html%00",
        ] {
            let (status, _, _) = get(&server, &format!("{root}{bad}"), &host(&server));
            assert_eq!(status, 404, "{bad}");
        }
    }

    #[test]
    fn web_invoke_runs_the_page_s_four_commands_with_the_core_s_refusals() {
        // The risk: a fifth command reachable over http, or one of the four
        // reachable with a check the window's IPC would have applied
        // skipped: every refusal here is the core's, before any child.
        let (server, _) = quiet();
        let root = format!("/{}/", server.token());
        let (status, answer) = post(
            &server,
            &format!("{root}invoke/get_trails"),
            r#"{"limit": 0}"#,
        );
        assert_eq!(status, 400);
        assert!(
            answer["error"]
                .as_str()
                .unwrap()
                .contains("limit must be 1 to 200"),
            "{answer}"
        );
        let (status, answer) = post(
            &server,
            &format!("{root}invoke/rpc_send"),
            r#"{"line": {"type": "quit"}}"#,
        );
        assert_eq!(status, 400);
        assert!(
            answer["error"].as_str().unwrap().contains("quit"),
            "{answer}"
        );
        let (status, answer) = post(
            &server,
            &format!("{root}invoke/rpc_send"),
            r#"{"line": {"type": "get_state"}}"#,
        );
        assert_eq!(status, 400);
        assert!(
            answer["error"]
                .as_str()
                .unwrap()
                .contains("no trellis rpc child"),
            "{answer}"
        );
        let (status, answer) = post(
            &server,
            &format!("{root}invoke/open_repository"),
            r#"{"path": "/nowhere-at-all"}"#,
        );
        assert_eq!(status, 400);
        assert!(
            answer["error"]
                .as_str()
                .unwrap()
                .contains("not a repository"),
            "{answer}"
        );
        let (status, answer) = post(&server, &format!("{root}invoke/get_overview"), "{}");
        assert_eq!(status, 400, "{answer}: a trellis that is not there");
        assert!(
            answer["error"]
                .as_str()
                .unwrap()
                .starts_with("get_overview:"),
            "{answer}"
        );
        let (status, answer) = post(&server, &format!("{root}invoke/quit"), "{}");
        assert_eq!(status, 404);
        assert_eq!(answer["error"], "no such command: quit");
        let (status, answer) = post(&server, &format!("{root}invoke/rpc_send"), "[]");
        assert_eq!(
            (status, answer["error"].as_str()),
            (400, Some("the arguments are a JSON object"))
        );
        let (status, _, _) = get(
            &server,
            &format!("{root}invoke/get_overview"),
            &host(&server),
        );
        assert_eq!(status, 405, "a command is a POST");
        let (status, _, _) = request(
            &server,
            &format!(
                "POST {root}invoke/get_overview HTTP/1.1\r\nHost: {}\r\n\r\n",
                host(&server)
            ),
        );
        assert_eq!(status, 411, "no Content-Length");
        assert_eq!(COMMANDS.len(), 4);
    }

    #[test]
    fn web_the_first_event_stream_runs_startup_once_and_hears_what_the_core_emits() {
        // The risk: startup running before any page listens, so its
        // `no-child` or `overview` is heard by nobody; running again on a
        // reload; or an event reaching one page and not another.
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&ran);
        let (server, sink) = server(Box::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        let root = format!("/{}/", server.token());
        let open = |server: &Server| {
            let mut s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            s.write_all(
                format!(
                    "GET {root}events HTTP/1.1\r\nHost: {}\r\n\r\n",
                    host(server)
                )
                .as_bytes(),
            )
            .unwrap();
            BufReader::new(s)
        };
        let mut first = open(&server);
        let mut line = String::new();
        first.read_line(&mut line).unwrap();
        assert!(line.starts_with("HTTP/1.1 200"), "{line}");
        let until = |r: &mut BufReader<TcpStream>, want: &str| {
            let mut got = String::new();
            loop {
                let mut l = String::new();
                r.read_line(&mut l).unwrap();
                got.push_str(&l);
                if got.contains(want) {
                    return got;
                }
                assert!(got.len() < 65_536, "no {want} in {got}");
            }
        };
        until(&mut first, ": connected");
        let started = std::time::Instant::now();
        while ran.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "startup never ran"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut second = open(&server);
        until(&mut second, ": connected");
        assert_eq!(
            ran.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a reload starts nothing twice"
        );
        assert_eq!(sink.open(), 2);
        sink.emit(
            "view",
            json!({"running": false, "note": "line one\nline two"}),
        );
        for r in [&mut first, &mut second] {
            let got = until(r, "\n\n");
            assert!(
                got.contains(
                    "event: view\ndata: {\"note\":\"line one\\nline two\",\"running\":false}\n\n"
                ),
                "{got}"
            );
        }
        // A page that went away is dropped at the next emit, not held.
        first.get_ref().shutdown(Shutdown::Both).unwrap();
        drop(first);
        std::thread::sleep(Duration::from_millis(200));
        for _ in 0..3 {
            sink.emit("overview-error", json!("tick"));
            std::thread::sleep(Duration::from_millis(50));
        }
        let got = until(&mut second, "data: \"tick\"\n");
        assert!(
            got.contains("event: overview-error\ndata: \"tick\"\n"),
            "{got}"
        );
    }

    #[test]
    fn web_sink_caps_bytes_per_stream() {
        // The risk (#814): a page that stopped reading held up to 1,024
        // events of any size each, so one stalled tab could hold the core's
        // memory by the gigabyte. A stream holds at most STREAM_BYTES of
        // unsent events and loses the oldest first, without the core waiting
        // on it; an event is never cut, since half of one is not an event
        // the page can read, so one past the cap on its own reaches no page.
        let sink = WebSink::default();
        let stalled = sink.subscribe();
        // Each frame is a quarter of the cap and a few bytes more: three
        // fit, and a fourth does not.
        let quarter = "x".repeat(STREAM_BYTES / 4);
        for i in 0..5 {
            sink.emit("overview-error", json!(format!("{i} {quarter}")));
            let held = stalled.unsent.lock().unwrap().bytes();
            assert!(held <= STREAM_BYTES, "{held} bytes held after event {i}");
        }
        assert_eq!(stalled.unsent.lock().unwrap().len(), 3);
        for i in 2..5 {
            let frame = stalled.next(Duration::ZERO).expect("a frame");
            assert!(
                frame.starts_with(&format!("event: overview-error\ndata: \"{i} x")),
                "the oldest go first: {}",
                &frame[..40]
            );
            assert!(frame.ends_with("x\"\n\n"), "a frame is never cut");
        }
        assert_eq!(stalled.next(Duration::ZERO), None);
        // Past the cap on its own: no page is sent it, cut or whole.
        sink.emit("overview-error", json!("y".repeat(STREAM_BYTES)));
        assert_eq!(stalled.next(Duration::ZERO), None);
        // The stream goes on with what comes next.
        sink.emit("view", json!({"running": true}));
        assert_eq!(
            stalled.next(Duration::ZERO).as_deref(),
            Some("event: view\ndata: {\"running\":true}\n\n")
        );
        assert_eq!(sink.open(), 1);
        // A stream whose page went away is dropped at the next emit.
        drop(stalled);
        sink.emit("view", json!({}));
        assert!(
            sink.streams.lock().unwrap().is_empty(),
            "a gone stream is held"
        );
    }

    #[test]
    fn web_js_is_the_one_module_that_fetches_and_only_from_its_own_path() {
        // The risk (ADR 0048 section 5): a module reaching the network, or
        // web.js reaching an origin other than the page's own; or web.js
        // replacing the window's IPC when Tauri is there.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        for entry in std::fs::read_dir(&dir).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            if !name.ends_with(".js") || name == "web.js" {
                continue;
            }
            let js = std::fs::read_to_string(dir.join(&name)).unwrap();
            for bad in [
                "fetch(",
                "EventSource",
                "XMLHttpRequest",
                "WebSocket",
                "navigator.sendBeacon",
            ] {
                assert!(!js.contains(bad), "{name} uses {bad}");
            }
        }
        let web = std::fs::read_to_string(dir.join("web.js")).unwrap();
        let code: String = web
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains("if (typeof window !== \"undefined\" && !window.__TAURI__)"),
            "web.js runs only without Tauri"
        );
        assert!(
            code.contains("new EventSource(\"events\")"),
            "the stream is the page's own events path"
        );
        assert!(
            code.contains("fetch(`invoke/${command}`"),
            "a command is the page's own invoke path"
        );
        for bad in [
            "http://",
            "https://",
            "ws://",
            "wss://",
            "//",
            "window.location",
            "document.cookie",
            "localStorage",
        ] {
            assert!(!code.contains(bad), "web.js holds {bad}");
        }
        for bad in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "Function(",
        ] {
            assert!(!code.contains(bad), "web.js uses {bad}");
        }
        let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
        let first = html.find("<script").expect("a script");
        assert!(
            html[first..].starts_with(r#"<script type="module" src="web.js"></script>"#),
            "web.js is the first module loaded"
        );
        // Every event name the core emits is one web.js hears from the
        // start, so an event heard before a module listens is kept for it.
        let emitted = emitted_events();
        let listed = code
            .split("const EVENTS = [")
            .nth(1)
            .expect("web.js lists the events")
            .split("];")
            .next()
            .unwrap();
        let listed: BTreeSet<String> = listed
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        assert_eq!(
            listed, emitted,
            "web.js's EVENTS is every name the core emits"
        );
    }

    /// Every event name an `emit(` in the core's sources names, tests
    /// included, whether the name follows on the same line or the next.
    fn emitted_events() -> BTreeSet<String> {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut emitted = BTreeSet::new();
        let mut files = vec![src_dir];
        while let Some(p) = files.pop() {
            if p.is_dir() {
                files.extend(std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()));
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                for part in text.split("emit(\"").skip(1) {
                    if let Some(name) = part.split('"').next() {
                        emitted.insert(name.to_string());
                    }
                }
                for part in text.split("emit(\n").skip(1) {
                    if let Some(name) = part
                        .trim_start()
                        .strip_prefix('"')
                        .and_then(|r| r.split('"').next())
                    {
                        emitted.insert(name.to_string());
                    }
                }
            }
        }
        emitted
    }

    #[test]
    fn every_emitted_event_has_a_listener_in_the_page() {
        // The risk (#821): the core emits an event nothing listens for, as
        // rpc-human and rpc-unparsed were, so the window carries it for
        // nothing and web.js keeps a queue of it for the page's lifetime.
        // Each name the core emits is one a page module (web.js aside, which
        // relays every event) passes to `listen`.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let mut heard = BTreeSet::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().is_none_or(|x| x != "js") || p.ends_with("web.js") {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            for part in text.split("listen(\"").skip(1) {
                if let Some(name) = part.split('"').next() {
                    heard.insert(name.to_string());
                }
            }
        }
        let unheard: Vec<String> = emitted_events().difference(&heard).cloned().collect();
        assert!(
            unheard.is_empty(),
            "the core emits {unheard:?}, which no page module listens for"
        );
    }

    fn ui(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("ui").join(name)
    }
}
