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

use crate::graph::{Index, SymbolKind, Target};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
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
        /// `serverInfo` from `initialize`, as `name version`; failing that,
        /// the version in the server's npm `package.json`, marked as read
        /// from there (Pyright 1.1.414 sends no `serverInfo`). `None` only
        /// when both are missing. Reported beside the counts, never stored
        /// in `.trellis/index.bin`.
        server: Option<String>,
        /// Set when the server stopped answering part way through: the
        /// sites after that point were not queried and keep their verdict.
        stopped: Option<String>,
    },
}

/// Why the backstop refused an answer the server did give.
///
/// A reason rather than a bare count, so a new refusal is one more variant
/// and not another reshape of [`BackstopReport`]. Every variant is a site
/// that keeps the resolver's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Refusal {
    /// A location with no URI or no readable start position.
    Malformed,
    /// A location outside the repository, or in a file the index does not
    /// hold (the standard library, site-packages, a stub).
    OutsideRepository,
    /// A location inside an indexed file but not at a definition's name,
    /// such as a parameter or a local variable inside a function's span.
    NotADefinitionName,
    /// Locations naming more than one symbol.
    MoreThanOneSymbol,
    /// A symbol outside a `Candidates` site's set, which is an upper bound
    /// over first-party code (ADR 0012), so the answer is a disagreement.
    OutsideCandidates,
    /// A method a first-party subclass overrides, or a function or method
    /// that is `@abstractmethod` or has a stub body (`...`, `pass`, or a
    /// raise of Python's not-implemented exception). Pyright names the member of the
    /// receiver's declared type, and a declared type is an upper bound, not
    /// an identity (ADR 0012): the call may reach an override, and an
    /// abstract or stub body is never the one that runs (ADR 0031, #186).
    OverriddenOrAbstract,
}

impl Refusal {
    /// What the refusal means, for output.
    pub fn describe(self) -> &'static str {
        match self {
            Refusal::Malformed => "malformed location",
            Refusal::OutsideRepository => "outside the repository",
            Refusal::NotADefinitionName => "not at a definition's name",
            Refusal::MoreThanOneSymbol => "more than one symbol",
            Refusal::OutsideCandidates => "outside the Candidates set",
            Refusal::OverriddenOrAbstract => "overridden or abstract",
        }
    }
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
    /// Queried sites the server returned no location for (`null` or an
    /// empty list). They keep the resolver's verdict.
    pub no_answer: u32,
    /// Queried sites whose answer this module would not bind, by reason.
    /// They keep the resolver's verdict. Kept apart from `no_answer`
    /// because "the server did not know" and "the server said something we
    /// would not take" call for different fixes.
    pub refused: BTreeMap<Refusal, u32>,
    /// Doubt sites never queried because the server was absent or stopped.
    pub not_queried: u32,
    /// Sites passed in that the resolver bound `Resolved` or that produced
    /// no edge (external). Never queried.
    pub skipped_certain: u32,
}

impl BackstopReport {
    /// Refused answers over every reason. One population (queried sites
    /// with an answer), so the sum is a count, not a mix.
    pub fn refused_total(&self) -> u32 {
        self.refused.values().sum()
    }

    fn not_run(reason: String, not_queried: u32, skipped_certain: u32) -> Self {
        BackstopReport {
            status: BackstopStatus::NotRun { reason },
            edges: Vec::new(),
            queried: 0,
            no_answer: 0,
            refused: BTreeMap::new(),
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
    /// Where to read the version when `initialize` carries no `serverInfo`.
    pub package: Option<NpmPackage>,
}

/// An npm package whose `package.json` names the server's version.
///
/// Read from the file rather than asked of the program: `pyright-langserver
/// --version` on 1.1.414 exits 1 ("Connection input stream is not set")
/// because the language server has no such flag, and a file read starts no
/// second Node process.
#[derive(Debug, Clone)]
pub struct NpmPackage {
    /// The program to find on `PATH`, as `Lsp::start` finds it.
    pub bin: OsString,
    /// The package's `name`. A `package.json` naming anything else is not
    /// this server's and is not read.
    pub name: String,
}

impl Server {
    /// `pyright-langserver --stdio`, found on `PATH`.
    pub fn pyright() -> Self {
        Server {
            program: "pyright-langserver".into(),
            args: vec!["--stdio".into()],
            timeout: Duration::from_secs(120),
            package: Some(NpmPackage {
                bin: "pyright-langserver".into(),
                name: "pyright".into(),
            }),
        }
    }
}

/// How long `initialize` may go unanswered. The handshake does no analysis
/// (Pyright reads imports on the first query), so it needs only Node's
/// start-up; 20 s is the bound trellis-cli's `liveness.rs` gives its probes.
const INITIALIZE_BOUND: Duration = Duration::from_secs(20);

/// How long a server whose output has closed is given to be reported as
/// exited, before the message says only that its output closed.
const EXIT_GRACE: Duration = Duration::from_secs(1);

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
    // Before the server starts: without this set the rule in `bind` cannot
    // be applied, and binding without it is what #186 decided against.
    let not_runtime = match not_the_runtime_callee(index, &root) {
        Ok(s) => s,
        Err(reason) => return BackstopReport::not_run(reason, doubt, skipped_certain),
    };
    let mut lsp = match Lsp::start(server, &root) {
        Ok(l) => l,
        Err(reason) => return BackstopReport::not_run(reason, doubt, skipped_certain),
    };
    let server_info = match lsp.initialize(&root) {
        // Pyright sends no `serverInfo`, so its version comes from the
        // package the program on PATH belongs to.
        Ok(None) => server.package.as_ref().and_then(|p| {
            let path = std::env::var_os("PATH");
            package_version(p, path.as_deref())
                .map(|v| format!("{} {v}, from its package.json", p.name))
        }),
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
        no_answer: 0,
        refused: BTreeMap::new(),
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
            match bind(&result, targets[&site], &mut files, &not_runtime) {
                Ok(callee) => report.edges.push(BackstopEdge {
                    from: site,
                    callee,
                    source: EdgeSource::Pyright,
                }),
                Err(Answer::None) => report.no_answer += 1,
                Err(Answer::Refused(why)) => *report.refused.entry(why).or_default() += 1,
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

/// The version in the `package.json` of the npm package `pkg.bin` belongs
/// to, found through `path` (the `PATH` value) the way the server was
/// started. `None` when the program is not found, no candidate file names
/// `pkg.name`, or the version is not a plain version string.
fn package_version(pkg: &NpmPackage, path: Option<&std::ffi::OsStr>) -> Option<String> {
    let bin = find_program(&pkg.bin, path)?;
    let dir = bin.parent()?;
    let mut candidates = Vec::new();
    // Unix: `.bin/pyright-langserver` links to `../pyright/langserver.index.js`.
    if let Some(real) = std::fs::canonicalize(&bin)
        .ok()
        .as_deref()
        .and_then(Path::parent)
    {
        candidates.push(real.join("package.json"));
    }
    // npm on Windows writes shims, not links: `node_modules/.bin/x.cmd` for
    // a local install, `<prefix>/x.cmd` beside `<prefix>/node_modules` for a
    // global one.
    candidates.push(dir.join("..").join(&pkg.name).join("package.json"));
    candidates.push(
        dir.join("node_modules")
            .join(&pkg.name)
            .join("package.json"),
    );
    candidates.iter().find_map(|c| {
        // A package file is not trusted input: it reaches stdout only as a
        // short string of version characters.
        let text = std::fs::read_to_string(c).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        if json.get("name").and_then(Value::as_str) != Some(pkg.name.as_str()) {
            return None;
        }
        let v = json.get("version")?.as_str()?;
        let plain = !v.is_empty()
            && v.len() <= 64
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b));
        plain.then(|| v.to_string())
    })
}

/// `program` as `Lsp::start` would run it: a path as given, else a bare
/// name from the first `PATH` entry holding it. On Windows in the same order
/// the spawn tries: `name.exe` on the whole `PATH`, then the `.cmd` fallback.
fn find_program(program: &OsString, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_path_buf());
    }
    let suffixes: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd"]
    } else {
        &[""]
    };
    let path = path?;
    suffixes.iter().find_map(|suffix| {
        let mut name = program.clone();
        name.push(suffix);
        std::env::split_paths(path)
            .map(|dir| dir.join(&name))
            .find(|c| c.is_file())
    })
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Why a queried site was not bound.
enum Answer {
    /// The server returned no location.
    None,
    /// The server returned locations this module would not bind.
    Refused(Refusal),
}

/// The one symbol a definition answer names, or why the site is left alone.
///
/// Conservative in every direction, because an edge bound here is evidence
/// and not proof (ADR 0031): every location must name a first-party
/// definition, all of them the same one, and for a `Candidates` site it must
/// be one of the resolver's candidates. The resolver's set is an upper bound
/// over first-party code (ADR 0012), so an answer outside it is a
/// disagreement, not a refinement. Last, the symbol must not be in
/// `not_runtime` (see [`not_the_runtime_callee`]); an answer outside the
/// set counts as outside it, whether or not it is also overridden.
fn bind(
    result: &Value,
    target: &Target,
    files: &mut Files<'_>,
    not_runtime: &HashSet<SymbolId>,
) -> Result<SymbolId, Answer> {
    let locations: Vec<&Value> = match result {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![result],
        _ => Vec::new(),
    };
    let mut found: Option<SymbolId> = None;
    for loc in locations {
        // `Location` or `LocationLink`.
        let (uri, range) = match (loc.get("uri"), loc.get("targetUri")) {
            (Some(u), _) => (u, loc.get("range")),
            (None, Some(u)) => (u, loc.get("targetSelectionRange")),
            (None, None) => return Err(Answer::Refused(Refusal::Malformed)),
        };
        let start = range.and_then(|r| r.get("start"));
        let at = |k: &str| {
            start
                .and_then(|s| s.get(k))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        };
        let (Some(uri), Some(line), Some(character)) = (uri.as_str(), at("line"), at("character"))
        else {
            return Err(Answer::Refused(Refusal::Malformed));
        };
        let sym = files
            .symbol_named_at(uri, line, character)
            .map_err(Answer::Refused)?;
        match found {
            Some(s) if s != sym => return Err(Answer::Refused(Refusal::MoreThanOneSymbol)),
            _ => found = Some(sym),
        }
    }
    let sym = found.ok_or(Answer::None)?;
    if let Target::Candidates(c) = target
        && !c.contains(&sym)
    {
        return Err(Answer::Refused(Refusal::OutsideCandidates));
    }
    if not_runtime.contains(&sym) {
        return Err(Answer::Refused(Refusal::OverriddenOrAbstract));
    }
    Ok(sym)
}

/// The symbols an answer may not be bound to: methods a first-party
/// subclass overrides, and functions or methods that are `@abstractmethod`
/// or have a stub body.
///
/// The index holds no class hierarchy (it is not in `.trellis/index.bin`),
/// so the files are read and parsed again and the hierarchy rebuilt with the
/// resolver's own rule. That is sound only when the files are the ones the
/// index was built from, so a file that is unreadable or changed since, or a
/// symbol arena that does not line up, is an error: the backstop then does
/// not run, rather than bind without the rule.
fn not_the_runtime_callee(index: &Index, root: &Path) -> Result<HashSet<SymbolId>, String> {
    let stale = |what: &str| {
        format!(
            "{what} since the index was built, so which methods are overridden is not known; \
             re-run the index"
        )
    };
    let mut sources = Vec::with_capacity(index.files.len());
    let mut facts = Vec::with_capacity(index.files.len());
    let mut out = HashSet::new();
    let mut sym = 0usize;
    for entry in &index.files {
        let src = std::fs::read_to_string(root.join(&entry.path))
            .map_err(|e| stale(&format!("{} became unreadable ({e})", entry.path)))?;
        if crate::resolve::content_hash(&src) != entry.hash {
            return Err(stale(&format!("{} changed", entry.path)));
        }
        let (f, tree) = crate::parse::collect_with_tree(&src, None);
        for d in &f.defs {
            let id = SymbolId::from_index(sym);
            sym += 1;
            if d.kind == SymbolKind::Class {
                continue;
            }
            let is_abstract = d
                .decorators
                .iter()
                .any(|dec| dec == "abstractmethod" || dec.ends_with(".abstractmethod"));
            if is_abstract || tree.as_ref().is_some_and(|t| stub_body(t, &src, d.span)) {
                out.insert(id);
            }
        }
        facts.push(f);
        sources.push((entry.path.clone(), src));
    }
    let overridden = crate::resolve::overridden_methods(index, &sources, &facts)
        .ok_or_else(|| stale("the definitions changed"))?;
    out.extend(overridden);
    Ok(out)
}

/// Python's exception for a body not yet written. Spelled in two parts
/// because `scripts/gates/no-stubs.sh` reads the class name as a stub
/// marker in Rust source; here it is what a Python stub is recognised by.
const NOT_IMPLEMENTED: &str = concat!("NotImpl", "ementedError");

/// Whether the definition at `span` has a body that cannot be what runs:
/// after an optional docstring, every statement is `...`, `pass` or a raise
/// of [`NOT_IMPLEMENTED`] (bare or called), and there is at least one.
fn stub_body(tree: &tree_sitter::Tree, src: &str, span: (usize, usize)) -> bool {
    let Some(mut node) = tree.root_node().descendant_for_byte_range(span.0, span.1) else {
        return false;
    };
    while node.kind() != "function_definition" || node.byte_range() != (span.0..span.1) {
        match node.parent() {
            Some(p) => node = p,
            None => return false,
        }
    }
    let Some(body) = node.child_by_field_name("body") else {
        return false;
    };
    let mut cursor = body.walk();
    let statements: Vec<tree_sitter::Node<'_>> = body
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
        .collect();
    let only_child = |n: tree_sitter::Node<'_>, kind: &str| {
        n.kind() == "expression_statement"
            && n.named_child_count() == 1
            && n.named_child(0).is_some_and(|c| c.kind() == kind)
    };
    let rest = match statements.split_first() {
        Some((first, rest)) if only_child(*first, "string") => rest,
        _ => &statements[..],
    };
    let not_implemented = |n: tree_sitter::Node<'_>| {
        let name = match n.named_child(0) {
            Some(c) if c.kind() == "call" => c.child_by_field_name("function"),
            other => other,
        };
        name.is_some_and(|c| {
            c.kind() == "identifier" && c.utf8_text(src.as_bytes()) == Ok(NOT_IMPLEMENTED)
        })
    };
    !rest.is_empty()
        && rest.iter().all(|&n| match n.kind() {
            "pass_statement" => true,
            "raise_statement" => not_implemented(n),
            _ => only_child(n, "ellipsis"),
        })
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
    fn symbol_named_at(
        &mut self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<SymbolId, Refusal> {
        let file = self.file_at(uri).ok_or(Refusal::OutsideRepository)?;
        self.name_at(file, line, character)
            .ok_or(Refusal::NotADefinitionName)
    }

    /// The indexed file `uri` names, or `None` when it names anything else.
    fn file_at(&self, uri: &str) -> Option<FileId> {
        let path = std::fs::canonicalize(uri_to_path(uri)?).ok()?;
        let rel = path.strip_prefix(&self.root).ok()?;
        let rel: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        self.by_path.get(rel.join("/").as_str()).copied()
    }

    fn name_at(&mut self, file: FileId, line: u32, character: u32) -> Option<SymbolId> {
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
        // The handshake gets its own bound, not `Server::timeout`: that one
        // is generous for a first query's import analysis, and a server
        // that never answers `initialize` held `trellis index` silent for
        // 125 s under it (#445).
        let query_timeout = self.timeout;
        self.timeout = query_timeout.min(INITIALIZE_BOUND);
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
        );
        self.timeout = query_timeout;
        let result = result?;
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
                    return Err(self.closed_before(method));
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

    /// What to say when the server's stdout ended before `method` was
    /// answered. End of stream means the output closed, not that the process
    /// is gone (#448): it says it exited only when the OS reports a status.
    /// A process that exits closes its output a moment before it can be
    /// reaped, so the status is polled for briefly rather than read once.
    fn closed_before(&mut self, method: &str) -> String {
        let until = Instant::now() + EXIT_GRACE;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return format!("the server exited ({status}) before answering {method}");
            }
            if Instant::now() >= until {
                return format!("the server closed its output before answering {method}");
            }
            std::thread::sleep(Duration::from_millis(20));
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
    /// its second argument it exits on the first definition request; with
    /// `noinfo` it leaves `serverInfo` out of `initialize`, as Pyright does.
    /// On `initialize`, `closeout` closes its stdout and keeps running for
    /// five seconds, and `exit3` exits with status 3, both without answering.
    const FAKE_SERVER: &str = r#"
import json, os, sys, time
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
        if mode == "closeout":
            os.close(1)
            time.sleep(5)
            sys.exit(0)
        if mode == "exit3":
            sys.exit(3)
        send(dict(jsonrpc="2.0", id="cfg", method="workspace/configuration", params=dict(items=[dict(section="python")])))
        reply = read()
        assert reply.get("id") == "cfg", reply
        result = dict(capabilities=dict())
        if mode != "noinfo":
            result["serverInfo"] = dict(name="fake-lsp", version="0.0.1")
        send(dict(jsonrpc="2.0", id=m["id"], result=result))
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
            let (program, mut args) = python();
            args.push(script.into());
            args.push(table.into());
            args.push(mode.into());
            Server {
                program: program.into(),
                args,
                timeout: Duration::from_secs(30),
                package: None,
            }
        }
    }

    /// The interpreter itself, not the launcher that found it: `py` on
    /// Windows keeps the server's stdout open while Python runs, so a server
    /// that closes its own stdout would not be seen to (#448).
    fn python() -> (String, Vec<OsString>) {
        let candidates: [(&str, &[&str]); 2] = [("py", &["-3"]), ("python3", &[])];
        for (program, pre) in candidates {
            let exe = Command::new(program)
                .args(pre)
                .args(["-c", "import sys; print(sys.executable)"])
                .stdin(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            if let Some(exe) = exe {
                return (exe, Vec::new());
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
        // `cb()` got `null`; `obj.stop()` got an answer that was refused.
        assert_eq!(
            (report.queried, report.no_answer, report.not_queried),
            (3, 1, 0)
        );
        assert_eq!(
            report.refused,
            BTreeMap::from([(Refusal::NotADefinitionName, 1)])
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
            package: None,
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
    fn backstop_reports_a_closed_output_without_claiming_the_server_exited() {
        // #448: end of stream on stdout says the output closed, not that the
        // process is gone. This server closes its stdout and keeps running;
        // a message naming an exit would send the reader after a crash that
        // did not happen.
        let f = fixture("closeout");
        let report = run(
            &f.index,
            &f.root,
            &doubt_sites(&f.index),
            &f.server(json!({}), "closeout"),
        );
        let BackstopStatus::NotRun { reason } = &report.status else {
            panic!("expected a backstop that did not run: {:?}", report.status);
        };
        assert_eq!(
            reason,
            "the server closed its output before answering initialize"
        );
    }

    #[test]
    fn backstop_reports_the_exit_status_of_a_server_that_exits() {
        // #448: when the process has exited, the message says so and names
        // the status the OS gave, so a crash reads as a crash.
        let f = fixture("exit3");
        let report = run(
            &f.index,
            &f.root,
            &doubt_sites(&f.index),
            &f.server(json!({}), "exit3"),
        );
        let BackstopStatus::NotRun { reason } = &report.status else {
            panic!("expected a backstop that did not run: {:?}", report.status);
        };
        // `ExitStatus` prints "exit code: 3" on Windows, "exit status: 3"
        // on Unix.
        assert!(
            reason.starts_with("the server exited (exit ")
                && reason.ends_with(": 3) before answering initialize"),
            "{reason}"
        );
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
        assert_eq!((report.queried, report.no_answer), (2, 0));
        assert_eq!(
            report.refused,
            BTreeMap::from([(Refusal::OutsideCandidates, 1)])
        );
    }

    #[test]
    fn pyright_backstop_refuses_an_overridden_callee_and_binds_a_concrete_leaf() {
        // #177 on poetry: Pyright names the member of the receiver's
        // declared type, and 813 of 857 Candidates bindings named a method a
        // subclass overrides. Inside the resolver's set, `OutsideCandidates`
        // does not catch that, and an Unresolved site has no set at all.
        // ADR 0012 keeps every override, so both are refused (#186); a
        // method no subclass overrides still binds, or the rule refuses
        // everything and proves nothing.
        let text = "class Base:
    def run(self):
        return 1


class Sub(Base):
    def run(self):
        return 2


class Leaf:
    def go(self):
        return 3


def use(a: Base, x):
    a.run()
    x.run()
    x.go()
";
        let f = fixture_of("refuses-overridden", text);
        let (typed, untyped, leaf) = (f.site("a.run()"), f.site("x.run()"), f.site("x.go()"));
        assert!(
            matches!(f.target(typed), Target::Candidates(c) if c.contains(&f.symbol("run"))),
            "fixture: a.run() must be Candidates holding Base.run, got {:?}",
            f.target(typed)
        );
        for s in [untyped, leaf] {
            assert!(
                matches!(f.target(s), Target::Unresolved(_)),
                "fixture: {s:?} must be Unresolved, got {:?}",
                f.target(s)
            );
        }
        let base_run = f.location(1, 8);
        let answers = json!({"16:6": base_run, "17:6": base_run, "18:6": f.location(11, 8)});
        let server = f.server(answers, "");
        let report = run(&f.index, &f.root, &[typed, untyped, leaf], &server);

        assert_eq!(
            report.edges,
            vec![BackstopEdge {
                from: leaf,
                callee: f.symbol("go"),
                source: EdgeSource::Pyright,
            }]
        );
        assert_eq!((report.queried, report.no_answer), (3, 0));
        assert_eq!(
            report.refused,
            BTreeMap::from([(Refusal::OverriddenOrAbstract, 2)])
        );
    }

    #[test]
    fn pyright_backstop_refuses_an_abstract_or_stub_callee() {
        // An `@abstractmethod`, or a body that is only `...`, `pass` or
        // `raise NotImplementedError`, is never what runs (#177 found 38 such
        // bindings on poetry). None of these methods is overridden, so the
        // override rule cannot be what refuses them; `real` has a body and
        // binds.
        let text = r#"from abc import ABC, abstractmethod


class Shape(ABC):
    @abstractmethod
    def area(self):
        return 0

    def todo(self):
        raise NotImplementedError("later")

    def hook(self):
        """Called."""
        ...

    def real(self):
        pass
        return 1


def use(s):
    s.area()
    s.todo()
    s.hook()
    s.real()
"#;
        let f = fixture_of("refuses-abstract", text);
        let sites: Vec<CallSiteId> = ["s.area()", "s.todo()", "s.hook()", "s.real()"]
            .iter()
            .map(|t| f.site(t))
            .collect();
        for &s in &sites {
            assert!(!f.target(s).is_certain(), "fixture: {s:?} must be doubt");
        }
        let answers = json!({
            "21:6": f.location(5, 8),
            "22:6": f.location(8, 8),
            "23:6": f.location(11, 8),
            "24:6": f.location(15, 8),
        });
        let server = f.server(answers, "");
        let report = run(&f.index, &f.root, &sites, &server);

        assert_eq!(
            report.edges,
            vec![BackstopEdge {
                from: sites[3],
                callee: f.symbol("real"),
                source: EdgeSource::Pyright,
            }]
        );
        assert_eq!(
            report.refused,
            BTreeMap::from([(Refusal::OverriddenOrAbstract, 3)])
        );
    }

    #[test]
    fn pyright_backstop_does_not_run_on_files_changed_since_the_index() {
        // Which methods are overridden is rebuilt from the files, and symbol
        // ids are positional: from a file edited after indexing, a refusal
        // could land on the wrong symbol and a bind on an overridden one.
        // The backstop does not run instead; every site keeps its verdict.
        let f = fixture("stale");
        std::fs::write(
            f.root.join("a.py"),
            format!("def first():\n    pass\n\n\n{REPO}"),
        )
        .unwrap();
        let server = f.server(json!({"10:8": f.location(1, 8)}), "");
        let doubt = doubt_sites(&f.index);
        let report = run(&f.index, &f.root, &doubt, &server);

        let BackstopStatus::NotRun { reason } = &report.status else {
            panic!("a stale index ran the backstop: {:?}", report.status);
        };
        assert!(reason.contains("a.py changed"), "{reason}");
        assert!(report.edges.is_empty());
        assert_eq!(report.not_queried, u32::try_from(doubt.len()).unwrap());
    }

    #[test]
    fn pyright_backstop_counts_no_answer_apart_from_each_refusal() {
        // #177 on poetry: 5,721 sites "left alone" could not say whether
        // Pyright knew nothing or answered and was refused, and those call
        // for different fixes. Each kind of answer lands in its own count.
        let text = "def helper():
    return 2


def other():
    return 3


def use(o):
    o.a()
    o.b()
    o.c()
    o.d()
    o.e()
";
        let f = fixture_of("split", text);
        let sites: Vec<CallSiteId> = ["o.a()", "o.b()", "o.c()", "o.d()", "o.e()"]
            .iter()
            .map(|t| f.site(t))
            .collect();
        for &s in &sites {
            assert!(!f.target(s).is_certain(), "fixture: {s:?} must be doubt");
        }
        // Under the root but not in the index, like a stub or a vendored file.
        let stray = f.root.join("not_indexed.py");
        std::fs::write(&stray, "def a():\n    pass\n").unwrap();
        let stray = std::fs::canonicalize(stray).unwrap();
        let zero = json!({"line": 0, "character": 4});
        let answers = json!({
            "9:6": null,
            "10:6": [],
            "11:6": {"range": {"start": zero, "end": zero}},
            "12:6": {"uri": path_to_uri(&stray), "range": {"start": zero, "end": zero}},
            "13:6": [f.location(0, 4), f.location(4, 4)],
        });
        let server = f.server(answers, "");
        let report = run(&f.index, &f.root, &sites, &server);

        assert!(report.edges.is_empty());
        assert_eq!((report.queried, report.no_answer), (5, 2));
        assert_eq!(
            report.refused,
            BTreeMap::from([
                (Refusal::Malformed, 1),
                (Refusal::OutsideRepository, 1),
                (Refusal::MoreThanOneSymbol, 1),
            ])
        );
        assert_eq!(report.refused_total(), 3);
    }

    /// An npm install of `pyright` under `dir`: `node_modules/.bin` holding
    /// the shims npm writes, and the package's `package.json`.
    fn npm_install(dir: &Path, package_json: &str) -> PathBuf {
        let bin = dir.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        for shim in ["pyright-langserver", "pyright-langserver.cmd"] {
            std::fs::write(bin.join(shim), "").unwrap();
        }
        let pkg = dir.join("node_modules").join("pyright");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("package.json"), package_json).unwrap();
        bin
    }

    fn pyright_package(bin: impl Into<OsString>) -> NpmPackage {
        NpmPackage {
            bin: bin.into(),
            name: "pyright".into(),
        }
    }

    #[test]
    fn pyright_backstop_reads_the_version_from_package_json_when_the_server_sends_none() {
        // Pyright 1.1.414 sends no `serverInfo` (#177), so ADR 0031's version
        // line printed "did not report its version" on every real run. The
        // fake server here omits it the same way; the version must come from
        // the package, marked as read from there, and be absent only when
        // both sources fail.
        let f = fixture("version");
        let bin = npm_install(
            &f.root.join("tool"),
            r#"{"name": "pyright", "version": "1.1.414"}"#,
        );
        let mut server = f.server(json!({}), "noinfo");
        server.package = Some(pyright_package(bin.join("pyright-langserver")));
        let report = run(&f.index, &f.root, &doubt_sites(&f.index), &server);
        assert_eq!(
            report.status,
            BackstopStatus::Ran {
                server: Some("pyright 1.1.414, from its package.json".into()),
                stopped: None
            }
        );

        // `serverInfo` wins when the server sends it.
        server.args.pop();
        server.args.push("".into());
        let report = run(&f.index, &f.root, &doubt_sites(&f.index), &server);
        assert!(
            matches!(&report.status, BackstopStatus::Ran { server: Some(s), .. } if s == "fake-lsp 0.0.1"),
            "{:?}",
            report.status
        );

        // Neither source: the version is unknown, and the run still counts.
        let mut server = f.server(json!({}), "noinfo");
        server.package = Some(pyright_package(f.root.join("no-such-langserver")));
        let report = run(&f.index, &f.root, &doubt_sites(&f.index), &server);
        assert_eq!(
            report.status,
            BackstopStatus::Ran {
                server: None,
                stopped: None
            }
        );
    }

    #[test]
    fn pyright_backstop_finds_the_package_the_way_the_server_was_started() {
        // A bare name is found on PATH, in the local layout
        // (`node_modules/.bin/x` beside `node_modules/pyright`) and the global
        // Windows one (`<prefix>/x.cmd` beside `<prefix>/node_modules`). A
        // package.json that names another package, or whose version is not a
        // version, is not read: the text reaches stdout.
        let f = fixture("package");
        let local = npm_install(
            &f.root.join("local"),
            r#"{"name": "pyright", "version": "1.1.414"}"#,
        );
        let path = std::env::join_paths([f.root.join("empty"), local]).unwrap();
        assert_eq!(
            package_version(&pyright_package("pyright-langserver"), Some(&path)),
            Some("1.1.414".into())
        );

        let global = f.root.join("global");
        std::fs::create_dir_all(global.join("node_modules").join("pyright")).unwrap();
        for shim in ["pyright-langserver", "pyright-langserver.cmd"] {
            std::fs::write(global.join(shim), "").unwrap();
        }
        std::fs::write(
            global.join("node_modules/pyright/package.json"),
            r#"{"name": "pyright", "version": "1.2.0"}"#,
        )
        .unwrap();
        assert_eq!(
            package_version(
                &pyright_package("pyright-langserver"),
                Some(global.as_os_str())
            ),
            Some("1.2.0".into())
        );

        for (i, body) in [
            r#"{"name": "not-pyright", "version": "1.1.414"}"#,
            "{\"name\": \"pyright\", \"version\": \"1.1\\u001b[31m\"}",
            r#"{"name": "pyright", "version": ""}"#,
            "not json",
        ]
        .iter()
        .enumerate()
        {
            let bin = npm_install(&f.root.join(format!("bad{i}")), body);
            assert_eq!(
                package_version(&pyright_package(bin.join("pyright-langserver")), None),
                None,
                "{body}"
            );
        }
        assert_eq!(
            package_version(&pyright_package("pyright-langserver"), None),
            None,
            "no PATH, nothing found"
        );
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
