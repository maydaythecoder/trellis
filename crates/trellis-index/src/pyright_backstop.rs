//! Pyright over LSP as the recall backstop (ADR 0031).
//!
//! The resolver gives every call site its verdict first. This module then
//! asks a language server, one `textDocument/definition` at a time, about the
//! sites the resolver left `Unresolved` or `Candidates`, and returns what the
//! server bound as edges tagged [`EdgeSource::Pyright`].
//!
//! What it deliberately does not do:
//!
//! - **Touch the graph.** The result is a separate report, never written into
//!   `Index::edges` or `Coverage`. Both are serialized into
//!   `.trellis/index.bin`, and the answer depends on the server's version,
//!   which the index file does not record. Keeping the edges out of the graph
//!   also keeps every such site unpartitionable (rule 6, ADR 0005):
//!   `Index::is_partitionable` reads only the resolver's edges, and ADR 0031
//!   leaves that question to a later decision.
//! - **Query a site the resolver bound or classified as external.** The
//!   filter is applied here, against the index's own edges, so a caller that
//!   passes the wrong list cannot widen the trigger.
//! - **Fail.** A server that is missing, cannot start, stops answering or
//!   exits leaves every site it did not answer with the resolver's verdict,
//!   and the report says why it did not run or where it stopped.
//!
//! The LSP framing is written here over the child's pipes rather than taken
//! from a crate: it is two headers and a JSON body, and ADR 0031 adds no Rust
//! dependency for Pyright.

use crate::graph::{Index, Target};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};
use trellis_core::{CallSiteId, FileId, SymbolId};

/// Who bound an edge.
///
/// Every edge in `Index::edges` is the resolver's. An edge the backstop
/// returns carries `Pyright`, so no consumer can count it as the resolver's
/// or fold it into `resolved` (ADR 0031, "Provenance").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeSource {
    /// The import-scoped resolver in this crate.
    Resolver,
    /// A `textDocument/definition` answer from the language server.
    Pyright,
}

/// One call site the backstop bound, and to what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackstopEdge {
    pub from: CallSiteId,
    pub callee: SymbolId,
    pub source: EdgeSource,
}

/// Whether the backstop ran, and if not, why.
///
/// Three states, so that "the server ran and bound nothing" never reads the
/// same as "the server was not there" (ADR 0031, "Absence degrades").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackstopStatus {
    /// No server was asked anything. Every site keeps its verdict.
    NotRun { reason: String },
    /// The server answered `initialize`.
    Ran {
        /// `serverInfo` from `initialize`, as `name version`, or `None` when
        /// the server did not say. Reported beside the counts, never stored
        /// in `.trellis/index.bin`.
        server: Option<String>,
        /// Set when the server stopped answering part way through: the
        /// sites after that point were not queried and keep their verdict.
        stopped: Option<String>,
    },
}

/// What the backstop did, as counts beside the resolver's and never merged
/// into them (CLAUDE.md rule 3: counts, not a fraction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackstopReport {
    pub status: BackstopStatus,
    /// The edges the server bound. Their count is the backstop-bound figure.
    pub edges: Vec<BackstopEdge>,
    /// Doubt sites a definition request was sent for.
    pub queried: u32,
    /// Queried sites the backstop left with the resolver's verdict: the
    /// server returned nothing, or an answer this module would not bind
    /// (outside the repository, not a definition's name, more than one
    /// symbol, or outside a `Candidates` site's set).
    pub left: u32,
    /// Doubt sites never queried because the server was absent or stopped.
    pub not_queried: u32,
    /// Sites passed in that the resolver bound `Resolved` or that produced
    /// no edge (external). Never queried.
    pub skipped_certain: u32,
}

impl BackstopReport {
    fn not_run(reason: String, not_queried: u32, skipped_certain: u32) -> Self {
        BackstopReport {
            status: BackstopStatus::NotRun { reason },
            edges: Vec::new(),
            queried: 0,
            left: 0,
            not_queried,
            skipped_certain,
        }
    }
}

/// The language server to start.
#[derive(Debug, Clone)]
pub struct Server {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// How long one request may go unanswered before the backstop stops.
    /// Pyright analyses a file's imports on its first query, which on a
    /// large repository takes a while, so the default is generous; a server
    /// that never answers still cannot hang `trellis index`.
    pub timeout: Duration,
}

impl Server {
    /// `pyright-langserver --stdio`, found on `PATH`.
    pub fn pyright() -> Self {
        Server {
            program: "pyright-langserver".into(),
            args: vec!["--stdio".into()],
            timeout: Duration::from_secs(120),
        }
    }
}

/// The sites the backstop may be asked about: every call site whose edge the
/// resolver left `Unresolved` or `Candidates`, in site order.
pub fn doubt_sites(index: &Index) -> Vec<CallSiteId> {
    let mut out: Vec<CallSiteId> = index
        .edges
        .iter()
        .filter(|e| !e.target.is_certain())
        .map(|e| e.from)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Asks `server` about each of `sites` that the resolver left `Unresolved`
/// or `Candidates`, and returns the edges it bound. Never fails: see the
/// module documentation.
pub fn run(index: &Index, root: &Path, sites: &[CallSiteId], server: &Server) -> BackstopReport {
    let targets: HashMap<CallSiteId, &Target> =
        index.edges.iter().map(|e| (e.from, &e.target)).collect();
    let mut by_file: BTreeMap<FileId, Vec<CallSiteId>> = BTreeMap::new();
    let mut skipped_certain = 0u32;
    let mut wanted = sites.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    for site in wanted {
        match targets.get(&site) {
            Some(t) if !t.is_certain() => match index.sites.get(site.index()) {
                Some(entry) => by_file.entry(entry.file).or_default().push(site),
                None => skipped_certain += 1,
            },
            // `Resolved`, or no edge at all (external): ADR 0031 never
            // queries these.
            _ => skipped_certain += 1,
        }
    }
    let doubt = count(by_file.values().map(Vec::len).sum());
    if doubt == 0 {
        return BackstopReport::not_run(
            "no Unresolved or Candidates site to query".into(),
            0,
            skipped_certain,
        );
    }

    let root = match std::fs::canonicalize(root) {
        Ok(r) => r,
        Err(e) => {
            return BackstopReport::not_run(
                format!("repository root unreadable: {e}"),
                doubt,
                skipped_certain,
            );
        }
    };
    let mut lsp = match Lsp::start(server, &root) {
        Ok(l) => l,
        Err(reason) => return BackstopReport::not_run(reason, doubt, skipped_certain),
    };
    let server_info = match lsp.initialize(&root) {
        Ok(info) => info,
        Err(reason) => {
            lsp.finish();
            return BackstopReport::not_run(reason, doubt, skipped_certain);
        }
    };

    let mut files = Files::new(index, &root);
    let mut report = BackstopReport {
        status: BackstopStatus::Ran {
            server: server_info,
            stopped: None,
        },
        edges: Vec::new(),
        queried: 0,
        left: 0,
        not_queried: 0,
        skipped_certain,
    };
    let mut stopped: Option<String> = None;
    for (file, file_sites) in &by_file {
        if stopped.is_some() {
            report.not_queried += count(file_sites.len());
            continue;
        }
        let Some((uri, src, tree)) = files.open(*file) else {
            // A file that cannot be read now cannot be asked about; its
            // sites keep their verdict.
            report.not_queried += count(file_sites.len());
            continue;
        };
        let opened = lsp.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "python", "version": 1, "text": src}}),
        );
        if let Err(e) = opened {
            stopped = Some(e);
            report.not_queried += count(file_sites.len());
            continue;
        }
        for (i, &site) in file_sites.iter().enumerate() {
            let entry = &index.sites[site.index()];
            let Some((line, character)) = callee_position(&tree, &src, entry.span) else {
                // `(a or b)()`: no identifier to ask about.
                report.not_queried += 1;
                continue;
            };
            let answer = lsp.request(
                "textDocument/definition",
                json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}}),
            );
            let result = match answer {
                Ok(v) => v,
                Err(e) => {
                    stopped = Some(e);
                    report.not_queried += count(file_sites.len() - i);
                    break;
                }
            };
            report.queried += 1;
            match bind(&result, targets[&site], &mut files) {
                Some(callee) => report.edges.push(BackstopEdge {
                    from: site,
                    callee,
                    source: EdgeSource::Pyright,
                }),
                None => report.left += 1,
            }
        }
        if stopped.is_none()
            && let Err(e) = lsp.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": uri}}),
            )
        {
            stopped = Some(e);
        }
    }
    if let BackstopStatus::Ran { stopped: s, .. } = &mut report.status {
        *s = stopped;
    }
    lsp.finish();
    report
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The one symbol a definition answer names, or `None` to leave the site
/// alone.
///
/// Conservative in every direction, because an edge bound here is evidence
/// and not proof (ADR 0031): every location must name a first-party
/// definition, all of them the same one, and for a `Candidates` site it must
/// be one of the resolver's candidates. The resolver's set is an upper bound
/// over first-party code (ADR 0012), so an answer outside it is a
/// disagreement, not a refinement.
fn bind(result: &Value, target: &Target, files: &mut Files<'_>) -> Option<SymbolId> {
    let locations: Vec<&Value> = match result {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![result],
        _ => Vec::new(),
    };
    let mut found: Option<SymbolId> = None;
    for loc in locations {
        // `Location` or `LocationLink`.
        let (uri, range) = match (loc.get("uri"), loc.get("targetUri")) {
            (Some(u), _) => (u, loc.get("range")?),
            (None, Some(u)) => (u, loc.get("targetSelectionRange")?),
            (None, None) => return None,
        };
        let start = range.get("start")?;
        let line = u32::try_from(start.get("line")?.as_u64()?).ok()?;
        let character = u32::try_from(start.get("character")?.as_u64()?).ok()?;
        let sym = files.symbol_named_at(uri.as_str()?, line, character)?;
        match found {
            Some(s) if s != sym => return None,
            _ => found = Some(sym),
        }
    }
    let sym = found?;
    match target {
        Target::Candidates(c) if !c.contains(&sym) => None,
        _ => Some(sym),
    }
}

/// Repository files the backstop has read, by id and by URI.
struct Files<'a> {
    index: &'a Index,
    root: PathBuf,
    by_path: HashMap<&'a str, FileId>,
    sources: HashMap<FileId, String>,
    symbols: HashMap<FileId, Vec<SymbolId>>,
}

impl<'a> Files<'a> {
    fn new(index: &'a Index, root: &Path) -> Self {
        let mut symbols: HashMap<FileId, Vec<SymbolId>> = HashMap::new();
        for (i, s) in index.symbols.iter().enumerate() {
            symbols
                .entry(s.file)
                .or_default()
                .push(SymbolId::from_index(i));
        }
        Files {
            index,
            root: root.to_path_buf(),
            by_path: index
                .files
                .iter()
                .enumerate()
                .map(|(i, f)| (f.path.as_str(), FileId::from_index(i)))
                .collect(),
            sources: HashMap::new(),
            symbols,
        }
    }

    fn source(&mut self, file: FileId) -> Option<&str> {
        if !self.sources.contains_key(&file) {
            let entry = self.index.files.get(file.index())?;
            let text = std::fs::read_to_string(self.root.join(&entry.path)).ok()?;
            self.sources.insert(file, text);
        }
        self.sources.get(&file).map(String::as_str)
    }

    /// The file's URI, its text and its parse, for `didOpen` and positions.
    fn open(&mut self, file: FileId) -> Option<(String, String, tree_sitter::Tree)> {
        let path = self.root.join(&self.index.files.get(file.index())?.path);
        let src = self.source(file)?.to_string();
        let tree = crate::parse::parser().parse(&src, None)?;
        Some((path_to_uri(&path), src, tree))
    }

    /// The symbol whose *name* starts at `(line, character)` in the file
    /// `uri` names, if that file is in the index.
    ///
    /// The name, not merely a position inside the definition: a location
    /// pointing at a local variable or a parameter lies inside the enclosing
    /// function's span, and binding it to that function would be an edge to
    /// the wrong callee.
    fn symbol_named_at(&mut self, uri: &str, line: u32, character: u32) -> Option<SymbolId> {
        let path = std::fs::canonicalize(uri_to_path(uri)?).ok()?;
        let rel = path.strip_prefix(&self.root).ok()?;
        let rel: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let file = *self.by_path.get(rel.join("/").as_str())?;
        let offset = offset_of(self.source(file)?, line, character)?;
        let src = self.sources.get(&file)?;
        let index = self.index;
        self.symbols.get(&file)?.iter().copied().find(|&s| {
            let entry = &index.symbols[s.index()];
            let name = index.interner.resolve(entry.name);
            let (start, end) = entry.span;
            if offset < start || offset + name.len() > end {
                return false;
            }
            let after = src.as_bytes().get(offset + name.len()).copied();
            let head = &src[start..offset];
            src[offset..].starts_with(name)
                && !after.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
                && head
                    .split_whitespace()
                    .all(|w| matches!(w, "def" | "class" | "async"))
                && !head.trim().is_empty()
        })
    }
}

/// Where to ask about a call: the start of the identifier it calls, so
/// `obj.run()` is asked at `run` and `f()` at `f`. `None` for a callee with
/// no identifier.
fn callee_position(
    tree: &tree_sitter::Tree,
    src: &str,
    span: (usize, usize),
) -> Option<(u32, u32)> {
    let mut node = tree.root_node().descendant_for_byte_range(span.0, span.1)?;
    while node.kind() != "call" || node.byte_range() != (span.0..span.1) {
        node = node.parent()?;
    }
    let callee = node.child_by_field_name("function")?;
    let name = match callee.kind() {
        "identifier" => callee,
        "attribute" => callee.child_by_field_name("attribute")?,
        _ => return None,
    };
    position_of(src, name.start_byte())
}

/// An LSP position (zero-based line, UTF-16 column) for a byte offset.
fn position_of(src: &str, offset: usize) -> Option<(u32, u32)> {
    let before = src.get(..offset)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line = before.matches('\n').count();
    let character: usize = before[line_start..].chars().map(char::len_utf16).sum();
    Some((u32::try_from(line).ok()?, u32::try_from(character).ok()?))
}

/// The byte offset of an LSP position, or `None` when it is not in `src`.
fn offset_of(src: &str, line: u32, character: u32) -> Option<usize> {
    let mut start = 0usize;
    for _ in 0..line {
        start += src.get(start..)?.find('\n')? + 1;
    }
    let text = &src[start..];
    let mut units = 0u32;
    for (i, c) in text.char_indices() {
        if units == character {
            return Some(start + i);
        }
        if c == '\n' {
            return None;
        }
        units += u32::try_from(c.len_utf16()).ok()?;
    }
    (units == character).then_some(src.len())
}

/// A `file://` URI for an absolute path. Bytes outside the unreserved set
/// are percent-encoded, so a space or `#` in a directory name survives.
fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    // `canonicalize` on Windows returns the verbatim form.
    let s = s.strip_prefix("//?/").unwrap_or(&s);
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The path a `file://` URI names, or `None` for any other scheme.
///
/// Servers disagree on the form (`file:///c%3A/x` and `file:///C:/x` are the
/// same file), so the caller canonicalizes before comparing.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(decoded).ok()?;
    // `/C:/x` is a Windows drive path; the leading slash belongs to the URI.
    let b = s.as_bytes();
    let s = if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        &s[1..]
    } else {
        &s[..]
    };
    Some(PathBuf::from(s))
}

/// A running language server and the JSON-RPC conversation with it.
struct Lsp {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    next_id: u64,
    timeout: Duration,
}

impl Lsp {
    fn start(server: &Server, root: &Path) -> Result<Self, String> {
        let mut child = match spawn(&server.program, &server.args, root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && cfg!(windows) => {
                // npm installs `pyright-langserver.cmd`, which a bare name
                // does not find on Windows.
                let mut cmd = server.program.clone();
                cmd.push(".cmd");
                spawn(&cmd, &server.args, root)
            }
            other => other,
        }
        .map_err(|e| {
            format!(
                "{} could not be started: {e}",
                server.program.to_string_lossy()
            )
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the server's pipes could not be opened".into());
        };
        let (tx, rx) = channel();
        // A reader thread, so a server that goes quiet is a timeout here
        // rather than a blocked read that hangs the index.
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            while let Ok(Some(msg)) = read_message(&mut r) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        Ok(Lsp {
            child,
            stdin: Some(stdin),
            rx,
            next_id: 1,
            timeout: server.timeout,
        })
    }

    /// `initialize` then `initialized`. Returns `serverInfo` as
    /// `name version`, when the server gave one.
    fn initialize(&mut self, root: &Path) -> Result<Option<String>, String> {
        let uri = path_to_uri(root);
        let result = self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": uri,
                "workspaceFolders": [{"uri": uri, "name": "root"}],
                "capabilities": {
                    "textDocument": {"definition": {"linkSupport": false}},
                },
            }),
        )?;
        self.notify("initialized", json!({}))?;
        let info = result.get("serverInfo");
        let name = info.and_then(|i| i.get("name")).and_then(Value::as_str);
        let version = info.and_then(|i| i.get("version")).and_then(Value::as_str);
        Ok(match (name, version) {
            (Some(n), Some(v)) => Some(format!("{n} {v}")),
            (None, Some(v)) => Some(v.to_string()),
            _ => None,
        })
    }

    fn send(&mut self, msg: &Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("the server's input is closed")?;
        write_message(stdin, msg).map_err(|e| format!("the server stopped reading: {e}"))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Sends a request and waits for its response, answering whatever the
    /// server asks in the meantime. Pyright asks `workspace/configuration`
    /// and registers capabilities, and waits for the replies.
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let msg = match self.rx.recv_timeout(left) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "no answer to {method} within {}s",
                        self.timeout.as_secs()
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("the server exited before answering {method}"));
                }
            };
            if let (Some(their), Some(asked)) = (msg.get("id"), msg.get("method")) {
                let reply = match asked.as_str() {
                    Some("workspace/configuration") => {
                        let n = msg
                            .pointer("/params/items")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len);
                        Value::Array(vec![Value::Null; n])
                    }
                    _ => Value::Null,
                };
                self.send(&json!({"jsonrpc": "2.0", "id": their, "result": reply}))?;
                continue;
            }
            if msg.get("id").and_then(Value::as_u64) != Some(id) {
                // A notification (diagnostics, logs) or a stale reply.
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(format!("{method} failed: {err}"));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// `shutdown` and `exit`, then make sure the process is gone.
    fn finish(mut self) {
        let shutdown_timeout = self.timeout.min(Duration::from_secs(5));
        self.timeout = shutdown_timeout;
        if self.request("shutdown", Value::Null).is_ok() {
            let _ = self.notify("exit", Value::Null);
        }
        drop(self.stdin.take());
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts the server. The one spawn in this crate, so the one place the key
/// variables are removed (ADR 0024 §3): a language server has no use for a
/// hosted model's key, and a crash dump or a log line could record it.
fn spawn(program: &OsString, args: &[OsString], root: &Path) -> std::io::Result<Child> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Not piped: nothing reads it, and a full pipe would stall the server.
        .stderr(Stdio::null());
    trellis_core::secrets::scrub(&mut cmd);
    cmd.spawn()
}

/// Largest message body accepted, so a server that sends a nonsense length
/// cannot make the index allocate without bound.
const MAX_BODY: usize = 64 << 20;

/// One LSP message: headers, a blank line, then `Content-Length` bytes of
/// JSON. `Ok(None)` at end of stream.
fn read_message(r: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let bad = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidData, m);
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse().ok();
        }
    }
    let n = len.ok_or_else(|| bad("a message with no Content-Length".into()))?;
    if n > MAX_BODY {
        return Err(bad(format!("a {n}-byte message")));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| bad(e.to_string()))
}

fn write_message(w: &mut impl Write, msg: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_from_sources, read_sources};

    /// A language server in thirty lines of Python, speaking LSP over stdio.
    /// It answers `textDocument/definition` from a JSON table keyed by
    /// `line:character`, and asks `workspace/configuration` during
    /// `initialize` the way Pyright does, so the client must answer a
    /// server-to-client request before its own reply arrives. With `die` as
    /// its second argument it exits on the first definition request.
    const FAKE_SERVER: &str = r#"
import json, sys
inp, out = sys.stdin.buffer, sys.stdout.buffer
answers = json.load(open(sys.argv[1]))
mode = sys.argv[2] if len(sys.argv) > 2 else ""
def read():
    n = None
    while True:
        line = inp.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        k, v = line.split(b":", 1)
        if k.strip().lower() == b"content-length":
            n = int(v)
    return json.loads(inp.read(n))
def send(msg):
    body = json.dumps(msg).encode()
    out.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    out.flush()
while True:
    m = read()
    if m is None:
        break
    meth = m.get("method")
    if meth == "initialize":
        send(dict(jsonrpc="2.0", id="cfg", method="workspace/configuration", params=dict(items=[dict(section="python")])))
        reply = read()
        assert reply.get("id") == "cfg", reply
        send(dict(jsonrpc="2.0", id=m["id"], result=dict(capabilities=dict(), serverInfo=dict(name="fake-lsp", version="0.0.1"))))
    elif meth == "textDocument/definition":
        if mode == "die":
            sys.exit(0)
        p = m["params"]["position"]
        send(dict(jsonrpc="2.0", id=m["id"], result=answers.get("%d:%d" % (p["line"], p["character"]))))
    elif meth == "shutdown":
        send(dict(jsonrpc="2.0", id=m["id"], result=None))
    elif meth == "exit":
        break
"#;

    // Zero-based lines: `obj.run()` is line 10 with `run` at column 8, and
    // `run`'s name is at 1:8, `helper`'s at 5:4.
    const REPO: &str = "class Base:
    def run(self):
        return 1


def helper():
    return 2


def use(obj, cb):
    obj.run()
    cb()
    helper()
    obj.stop()
";

    struct Fixture {
        root: PathBuf,
        index: Index,
        sources: Vec<(String, String)>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(name: &str) -> Fixture {
        fixture_of(name, REPO)
    }

    fn fixture_of(name: &str, text: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "trellis-pyright-backstop-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.py"), text).unwrap();
        let sources = read_sources(&root);
        let index = build_from_sources(&sources);
        Fixture {
            root,
            index,
            sources,
        }
    }

    impl Fixture {
        fn site(&self, text: &str) -> CallSiteId {
            let i = self
                .index
                .sites
                .iter()
                .position(|s| {
                    let src = &self.sources[s.file.index()].1;
                    &src[s.span.0..s.span.1] == text
                })
                .unwrap_or_else(|| panic!("no call site `{text}`"));
            CallSiteId::from_index(i)
        }

        fn target(&self, site: CallSiteId) -> &Target {
            &self
                .index
                .edges
                .iter()
                .find(|e| e.from == site)
                .unwrap()
                .target
        }

        fn symbol(&self, name: &str) -> SymbolId {
            let i = self
                .index
                .symbols
                .iter()
                .position(|s| self.index.interner.resolve(s.name) == name)
                .unwrap();
            SymbolId::from_index(i)
        }

        fn location(&self, line: u32, character: u32) -> Value {
            let path = std::fs::canonicalize(self.root.join("a.py")).unwrap();
            let pos = json!({"line": line, "character": character});
            json!({"uri": path_to_uri(&path), "range": {"start": pos, "end": pos}})
        }

        /// The fake server, run by whichever Python launcher this machine
        /// has. No Python is a test failure, not a skip: a backstop test
        /// that passes without a server proves nothing.
        fn server(&self, answers: Value, mode: &str) -> Server {
            let script = self.root.join("fake_lsp.py");
            let table = self.root.join("answers.json");
            std::fs::write(&script, FAKE_SERVER).unwrap();
            std::fs::write(&table, answers.to_string()).unwrap();
            let (program, mut args): (&str, Vec<OsString>) = python();
            args.push(script.into());
            args.push(table.into());
            args.push(mode.into());
            Server {
                program: program.into(),
                args,
                timeout: Duration::from_secs(30),
            }
        }
    }

    fn python() -> (&'static str, Vec<OsString>) {
        let candidates: [(&str, &[&str]); 2] = [("py", &["-3"]), ("python3", &[])];
        for (program, pre) in candidates {
            let ok = Command::new(program)
                .args(pre)
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success());
            if ok {
                return (program, pre.iter().map(OsString::from).collect());
            }
        }
        panic!(
            "these tests run a fake language server in Python; neither `py -3` nor `python3` runs"
        );
    }

    #[test]
    fn pyright_backstop_binds_an_answered_site_and_leaves_a_silent_one() {
        // The first slice's whole claim: an Unresolved site the server
        // answers comes back bound and tagged as the server's, one it does
        // not answer keeps the resolver's verdict, and the graph itself is
        // not touched.
        let f = fixture("binds");
        let run_site = f.site("obj.run()");
        let cb_site = f.site("cb()");
        let stop_site = f.site("obj.stop()");
        for s in [run_site, cb_site, stop_site] {
            assert!(
                !f.target(s).is_certain(),
                "fixture: {s:?} must be a doubt site"
            );
        }
        let edges_before = f.index.edges.clone();

        // `obj.stop()` is answered with a position inside `run`'s body, not
        // at a definition's name: binding it to the enclosing `run` would be
        // an edge to the wrong callee.
        let answers = json!({
            "10:8": f.location(1, 8),
            "11:4": null,
            "13:8": f.location(2, 8),
        });
        let server = f.server(answers, "");
        let report = run(&f.index, &f.root, &doubt_sites(&f.index), &server);

        assert_eq!(
            report.status,
            BackstopStatus::Ran {
                server: Some("fake-lsp 0.0.1".into()),
                stopped: None
            }
        );
        assert_eq!(
            report.edges,
            vec![BackstopEdge {
                from: run_site,
                callee: f.symbol("run"),
                source: EdgeSource::Pyright,
            }]
        );
        assert_eq!((report.queried, report.left, report.not_queried), (3, 2, 0));
        assert_eq!(
            f.index.edges, edges_before,
            "the graph is not the backstop's to change"
        );
        assert!(
            !f.target(run_site).is_certain(),
            "a backstop-bound site stays unpartitionable"
        );
    }

    #[test]
    fn pyright_backstop_never_queries_a_site_the_resolver_bound() {
        // ADR 0031: Pyright is asked only about Unresolved and Candidates
        // sites. `helper()` is Resolved; the fake server would bind it if
        // asked, so an edge for it means the filter leaked.
        let f = fixture("certain");
        let helper_site = f.site("helper()");
        assert!(
            f.target(helper_site).is_certain(),
            "fixture: helper() must be Resolved"
        );
        let answers = json!({"12:4": f.location(5, 4), "10:8": f.location(1, 8)});
        let server = f.server(answers, "");
        let report = run(
            &f.index,
            &f.root,
            &[helper_site, f.site("obj.run()")],
            &server,
        );

        assert!(report.edges.iter().all(|e| e.from != helper_site));
        assert_eq!((report.queried, report.skipped_certain), (1, 1));

        // Handed only certain sites, it starts nothing at all.
        let only = run(&f.index, &f.root, &[helper_site], &server);
        assert!(matches!(only.status, BackstopStatus::NotRun { .. }));
        assert_eq!(only.queried, 0);
    }

    #[test]
    fn pyright_backstop_without_a_server_keeps_every_verdict() {
        // Node or Pyright absent: the index must not fail, every site keeps
        // its verdict, and the report says the backstop did not run, so
        // "ran and found nothing" and "was not there" read differently.
        let f = fixture("absent");
        let server = Server {
            program: "trellis-no-such-language-server".into(),
            args: vec!["--stdio".into()],
            timeout: Duration::from_secs(5),
        };
        let doubt = doubt_sites(&f.index);
        let report = run(&f.index, &f.root, &doubt, &server);

        let BackstopStatus::NotRun { reason } = &report.status else {
            panic!("an absent server reported {:?}", report.status);
        };
        assert!(reason.contains("could not be started"), "{reason}");
        assert!(report.edges.is_empty());
        assert_eq!(report.queried, 0);
        assert_eq!(report.not_queried, u32::try_from(doubt.len()).unwrap());
    }

    #[test]
    fn pyright_backstop_survives_a_server_that_exits_mid_run() {
        // A crash part way through is the same as absence for every site not
        // yet answered: the verdicts stand and the report says where it
        // stopped.
        let f = fixture("dies");
        let doubt = doubt_sites(&f.index);
        let server = f.server(json!({}), "die");
        let report = run(&f.index, &f.root, &doubt, &server);

        let BackstopStatus::Ran {
            stopped: Some(why), ..
        } = &report.status
        else {
            panic!("expected a run that stopped: {:?}", report.status);
        };
        assert!(why.contains("exited"), "{why}");
        assert!(report.edges.is_empty());
        assert_eq!(report.queried, 0);
        assert_eq!(report.not_queried, u32::try_from(doubt.len()).unwrap());
    }

    #[test]
    fn pyright_backstop_binds_a_candidates_site_only_inside_the_resolver_s_set() {
        // A declared type makes `a.run()` a `Candidates` site over `Base.run`
        // and every override (ADR 0012). That set is an upper bound over
        // first-party code, so an answer outside it is a disagreement and is
        // left alone; one inside it is bound, tagged as the server's.
        let text = "class Base:
    def run(self):
        return 1


class Sub(Base):
    def run(self):
        return 2


def helper():
    return 3


def use(a: Base, b: Base):
    a.run()
    b.run()
";
        let f = fixture_of("candidates", text);
        let (a_site, b_site) = (f.site("a.run()"), f.site("b.run()"));
        for s in [a_site, b_site] {
            assert!(
                matches!(f.target(s), Target::Candidates(_)),
                "fixture: {s:?} must be Candidates, got {:?}",
                f.target(s)
            );
        }
        let sub_start = text.find("class Sub").unwrap();
        let sub_run = f
            .index
            .symbols
            .iter()
            .position(|s| f.index.interner.resolve(s.name) == "run" && s.span.0 > sub_start)
            .map(SymbolId::from_index)
            .unwrap();
        let answers = json!({"15:6": f.location(10, 4), "16:6": f.location(6, 8)});
        let server = f.server(answers, "");
        let report = run(&f.index, &f.root, &[a_site, b_site], &server);

        assert_eq!(
            report.edges,
            vec![BackstopEdge {
                from: b_site,
                callee: sub_run,
                source: EdgeSource::Pyright,
            }]
        );
        assert_eq!((report.queried, report.left), (2, 1));
    }

    #[test]
    fn pyright_backstop_reads_both_forms_of_a_windows_file_uri() {
        // VS Code-style servers send `file:///c%3A/...`; the client sends
        // `file:///C:/...`. Both must name the same file, or every answer
        // on Windows would be read as outside the repository.
        assert_eq!(
            uri_to_path("file:///c%3A/Users/x/a.py"),
            Some(PathBuf::from("c:/Users/x/a.py"))
        );
        assert_eq!(
            uri_to_path("file:///home/x/my%20repo/a.py"),
            Some(PathBuf::from("/home/x/my repo/a.py"))
        );
        assert_eq!(uri_to_path("untitled:1"), None);
        let src = "é = 1\nx𝄞y()\n";
        let at = src.find('y').unwrap();
        // `𝄞` is two UTF-16 units: LSP columns are not bytes or chars.
        assert_eq!(position_of(src, at), Some((1, 3)));
        assert_eq!(offset_of(src, 1, 3), Some(at));
    }
}
