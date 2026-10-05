//! Pass 2 (the module map) and the bind step.
//!
//! Pass 2 resolves every import's module path against the filesystem.
//! Anything landing outside the repository root — stdlib, site-packages — is
//! discarded here and never enters the graph. That is what makes the approach
//! tractable: we are not analyzing Python, we are analyzing this repository's
//! own names (ADR 0001).
//!
//! The bind step walks each call site's scope chain outward and emits one of
//! `Resolved`, `Candidates`, or `Unresolved(Reason)` — never two of them
//! collapsed into one (ADR 0002).
//!
//! **Bias.** Every judgement call here is made in the direction that loses
//! recall rather than the direction that invents an edge. An unsound
//! `Resolved` fails v1.0 gate item 2 outright; a miss is a number in a
//! coverage report. They are not symmetric and the code does not treat them
//! as though they were.

use crate::coverage::Coverage;
use crate::graph::{
    CallSiteEntry, Edge, FileEntry, Index, Reason, SymbolEntry, SymbolKind, Target,
};
use crate::parse::{CalleeShape, FileFacts};
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use trellis_core::{CallSiteId, FileId, SymbolId};

/// How far a re-export chain is followed before giving up.
///
/// Following an `__init__.py` re-export is sound — each hop is a real import
/// statement — so the limit is about cost and cycles, not correctness. Chains
/// deeper than this report `ReexportUnfollowed` rather than guessing.
const MAX_REEXPORT_DEPTH: u32 = 4;

/// How many base classes up `lookup_method` searches for a method.
///
/// Cycles are already cut by its `visited` set, so this bounds cost on deep
/// hierarchies, not termination. A method defined further up than this is
/// reported `MethodDispatchUntyped` (an unsearched base is a non-exhaustive
/// miss), which costs recall and never emits an edge.
const MAX_INHERITANCE_DEPTH: u32 = 4;

/// Builtin type names, for deciding that an annotated receiver is not
/// first-party.
///
/// Only names that are *types* — a receiver annotated `str` has a callee in
/// the standard library, which is the judgement being made. Functions like
/// `len` never appear as an annotation, so including them would add nothing
/// and widen an exclusion list that should stay as narrow as it can be.
///
/// Exhaustiveness is not required and not claimed: a builtin missing from
/// this list leaves its call sites in the denominator as misses, which is the
/// safe direction. A *wrong* entry would exclude first-party calls, so the
/// list contains only names Python itself defines.
const BUILTIN_TYPES: &[&str] = &[
    "bool",
    "bytearray",
    "bytes",
    "complex",
    "dict",
    "float",
    "frozenset",
    "int",
    "list",
    "memoryview",
    "object",
    "range",
    "set",
    "str",
    "tuple",
    "type",
];

/// Decorators that leave the decorated definition reachable by a plain call.
///
/// The conservative default is that a decorator is **opaque**: `@foo` may
/// return something entirely unrelated, so a call to a decorated definition
/// reports `DecoratorIndirection` rather than an edge we cannot justify.
/// This allowlist is the exception — decorators whose call-through behaviour
/// is part of their published contract.
///
/// `property` and `cached_property` are deliberately **absent**. They turn
/// attribute access into a call, so `self.x()` calls the *result* of the
/// property, not the property body. Resolving those would be an unsound edge
/// of exactly the kind the gate forbids.
///
/// `contextmanager` and `asynccontextmanager` were listed here and were
/// **removed on 2026-09-19 after the audit caught the resulting unsound
/// edge**. The argument for keeping them was that calling a decorated
/// generator still reaches its body, so a change to the body still affects the
/// call site. That is true of the dependency and false of the call: the
/// decorator returns a factory, and calling it constructs a context manager
/// without running a line of the body. Same class of error as `property`,
/// which is why it belongs in the same exclusion.
///
/// Matched on the final dotted segment, before any call arguments, so
/// `functools.lru_cache(maxsize=None)` matches `lru_cache`.
const TRANSPARENT_DECORATORS: &[&str] = &[
    "staticmethod",
    "classmethod",
    "abstractmethod",
    "abstractclassmethod",
    "abstractstaticmethod",
    "override",
    "final",
    "overload",
    "wraps",
    "lru_cache",
    "cache",
    "dataclass",
    "runtime_checkable",
    "no_type_check",
];

/// Python builtins. A call to one of these is external — out of the
/// denominator, not a miss.
///
/// Completeness matters in one direction only: a builtin we omit becomes
/// `UnboundName`, which *understates* recall. That is the safe direction for
/// an honest number, but it is still noise, so the list is kept thorough.
const BUILTINS: &[&str] = &[
    "abs",
    "aiter",
    "anext",
    "all",
    "any",
    "ascii",
    "bin",
    "bool",
    "breakpoint",
    "bytearray",
    "bytes",
    "callable",
    "chr",
    "classmethod",
    "compile",
    "complex",
    "delattr",
    "dict",
    "dir",
    "divmod",
    "enumerate",
    "eval",
    "exec",
    "filter",
    "float",
    "format",
    "frozenset",
    "getattr",
    "globals",
    "hasattr",
    "hash",
    "help",
    "hex",
    "id",
    "input",
    "int",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "list",
    "locals",
    "map",
    "max",
    "memoryview",
    "min",
    "next",
    "object",
    "oct",
    "open",
    "ord",
    "pow",
    "print",
    "property",
    "range",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "slice",
    "sorted",
    "staticmethod",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "vars",
    "zip",
    "__import__",
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "BaseException",
    "BaseExceptionGroup",
    "BlockingIOError",
    "BrokenPipeError",
    "BufferError",
    "BytesWarning",
    "ChildProcessError",
    "ConnectionAbortedError",
    "ConnectionError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "DeprecationWarning",
    "EOFError",
    "EncodingWarning",
    "EnvironmentError",
    "Exception",
    "ExceptionGroup",
    "FileExistsError",
    "FileNotFoundError",
    "FloatingPointError",
    "FutureWarning",
    "GeneratorExit",
    "IOError",
    "ImportError",
    "ImportWarning",
    "IndentationError",
    "IndexError",
    "InterruptedError",
    "IsADirectoryError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "MemoryError",
    "ModuleNotFoundError",
    "NameError",
    "NotADirectoryError",
    "NotImplementedError",
    "OSError",
    "OverflowError",
    "PendingDeprecationWarning",
    "PermissionError",
    "ProcessLookupError",
    "RecursionError",
    "ReferenceError",
    "ResourceWarning",
    "RuntimeError",
    "RuntimeWarning",
    "StopAsyncIteration",
    "StopIteration",
    "SyntaxError",
    "SyntaxWarning",
    "SystemError",
    "SystemExit",
    "TabError",
    "TimeoutError",
    "TypeError",
    "UnboundLocalError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeError",
    "UnicodeTranslateError",
    "UnicodeWarning",
    "UserWarning",
    "ValueError",
    "Warning",
    "ZeroDivisionError",
    "True",
    "False",
    "None",
    "NotImplemented",
    "Ellipsis",
];

/// Directories never walked. `.trellis/work` holds per-partition worktrees,
/// which are copies of the repository and would double every count.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".trellis",
    ".venv",
    "venv",
    "node_modules",
    "__pycache__",
    ".tox",
    ".nox",
    ".mypy_cache",
    ".pytest_cache",
    "site-packages",
    ".eggs",
    "build",
    "dist",
];

/// What an import alias binds to, before the imported *name* is looked up.
///
/// Name lookup is deferred because it may need to follow a re-export chain
/// into another file's import table, which does not exist until every file
/// has been through Pass 2.
#[derive(Clone, Debug)]
enum ImportBinding {
    /// `from <first-party module> import name`.
    FromModule { file: FileId, name: String },
    /// `import a.b.c [as x]`, resolving inside the repository.
    ModuleRef(FileId),
    /// Resolves outside the repository root. Out of the denominator.
    External,
    /// The module path's root package is first-party but the module itself
    /// did not resolve to a file. A miss, not an exclusion.
    Unresolvable,
}

/// The result of following a binding to something nameable.
#[derive(Clone, Debug)]
enum Bound {
    Module(FileId),
    Symbol(SymbolId),
    Symbols(SmallVec<[SymbolId; 4]>),
    External,
    /// First-party, but the chain could not be followed to a definition.
    Unfollowed(Reason),
}

/// Per-file lookup tables. Indexed by `FileId`.
#[derive(Default)]
struct FileTables {
    /// `key(scope, name)` -> definitions with that name at that scope.
    defs: HashMap<String, SmallVec<[SymbolId; 2]>>,
    /// `key(scope, name)` for parameters, locals, loop and `with` targets.
    bindings: HashSet<String>,
    /// Import alias -> what it binds. A `Vec` because a conditional import
    /// (`try: from a import x / except ImportError: from b import x`) binds
    /// the same alias twice, and that is `Candidates`, not a coin flip.
    imports: HashMap<String, Vec<ImportBinding>>,
    has_star: bool,
    /// `key(scope, name)` -> the distinct types stated for that name.
    ///
    /// A `SmallVec` rather than a single type because a name can be annotated
    /// once and reassigned differently. More than one entry means the source
    /// disagrees with itself, and the resolver then declines to type it at
    /// all — picking one would be a guess wearing a fact's clothes.
    types: HashMap<String, SmallVec<[String; 2]>>,
}

/// What the resolver knows about a first-party class.
#[derive(Clone, Debug)]
struct ClassInfo {
    file: FileId,
    /// Scope path the class's own methods are keyed under: its scope plus its
    /// own name.
    path: Vec<String>,
    /// Base class expressions, as written.
    bases: Vec<String>,
}

/// The result of looking for a method on a class and its bases.
enum MethodLookup {
    /// `owner` is the class the definition was found on, which may be an
    /// ancestor of the class searched. The override set that matters hangs
    /// off the owner, not off the class we started from.
    One {
        method: SymbolId,
        owner: SymbolId,
    },
    Many(SmallVec<[SymbolId; 4]>),
    /// A `def` and a class-scope assignment share the name, so which one is
    /// live at the call depends on statement order and on what the assignment
    /// evaluates to. See `Reason::ShadowedAmbiguous` and Q-14.
    Shadowed,
    /// Not found. `exhaustive` is true only when every base class in the
    /// hierarchy was itself first-party and searched — otherwise the method
    /// may live in a third-party base and the absence proves nothing.
    NotFound {
        exhaustive: bool,
    },
}

/// Scope-qualified lookup key. `\u{1}` cannot appear in a Python identifier,
/// so the join is unambiguous.
fn key(scope: &[String], name: &str) -> String {
    let mut k = scope.join("::");
    k.push('\u{1}');
    k.push_str(name);
    k
}

/// Repository-relative path with forward slashes, whatever the platform.
fn rel_path(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Dotted module path for a repository-relative `.py` path.
///
/// A leading `src/` is stripped: the src-layout convention puts `poetry.x` at
/// `src/poetry/x.py`, and the benchmark repository uses it. `__init__.py`
/// names its package rather than a module inside it.
///
/// ASSUMPTION A-26: `src/` is the only synthetic package root recognised.
/// A repository using a different one resolves fewer imports, visibly, in
/// the coverage report — it does not silently mis-resolve them.
pub fn module_path_of(rel: &str) -> String {
    let stripped = rel.strip_suffix(".py").unwrap_or(rel);
    let stripped = stripped.strip_prefix("src/").unwrap_or(stripped);
    let stripped = stripped.strip_suffix("/__init__").unwrap_or(stripped);
    if stripped == "__init__" {
        return String::new();
    }
    stripped.replace('/', ".")
}

/// Every `.py` file under `root`, excluding vendored and generated trees.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            !e.file_type().is_dir()
                || !e
                    .file_name()
                    .to_str()
                    .is_some_and(|n| SKIP_DIRS.contains(&n))
        })
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|x| x == "py"))
        .collect();
    // Deterministic ids: the same repository must produce the same index,
    // or `insta` snapshots and committed trails are meaningless.
    out.sort();
    out
}

/// Builds the whole index for a repository: discover, Pass 1, Pass 2, bind.
#[tracing::instrument(skip_all, fields(root = %root.display()))]
pub fn build(root: &Path) -> Index {
    build_from_sources(&read_sources(root))
}

/// Every first-party Python file, as repository-relative paths.
///
/// The same walk the graph is built from, deliberately: a second walker in
/// another crate would drift from this one, and the drift would be silent —
/// a prompt naming a file the index does not know, or omitting one it does.
///
/// Separators are `/` on every platform, because these paths are handed to a
/// model that must quote them inside JSON, and a Windows `\` is either an
/// invalid escape or a lie about the path.
pub fn discover_relative(root: &Path) -> Vec<String> {
    discover(root).iter().map(|p| rel_path(root, p)).collect()
}

/// Reads every indexable file under `root` as a `(relative path, source)` pair.
pub fn read_sources(root: &Path) -> Vec<(String, String)> {
    discover(root)
        .iter()
        .filter_map(|p| {
            let src = std::fs::read_to_string(p).ok()?;
            Some((rel_path(root, p), src))
        })
        .collect()
}

/// Loads `.trellis/index.bin` when it matches the current file set, otherwise
/// builds and writes it.
///
/// The cache is checked against a fingerprint of the files just read, so a
/// stale index is discarded rather than returned. A failed write is not an
/// error the caller should care about: the index is derived state, and losing
/// the cache costs a rebuild, not correctness.
pub fn build_cached(root: &Path) -> Index {
    let sources = read_sources(root);
    let fp = crate::cache::fingerprint(&sources);
    let path = root.join(".trellis").join("index.bin");

    if let Some(index) = crate::cache::load(&path, fp) {
        tracing::debug!("index loaded from cache");
        return index;
    }
    let index = build_from_sources(&sources);
    if let Err(e) = crate::cache::save(&index, &path) {
        tracing::debug!("could not write index cache: {e}");
    }
    index
}

/// The resolver proper, over `(relative_path, source)` pairs.
///
/// Taking sources rather than a directory is what lets the whole resolver be
/// tested from fixtures with no filesystem layout to arrange: only `discover`
/// and `read_sources` touch the disk, and the resolver core never does.
#[tracing::instrument(skip_all, fields(files = sources.len()))]
pub fn build_from_sources(sources: &[(String, String)]) -> Index {
    use rayon::prelude::*;

    // ---- Pass 1: parse and collect, in parallel. ----
    let facts: Vec<FileFacts> = sources
        .par_iter()
        .enumerate()
        .map(|(i, (_, src))| crate::parse::collect(src, FileId::from_index(i)))
        .collect();

    build_from_facts(sources, &facts)
}

/// Pass 2 and the bind step, over facts that have already been collected.
///
/// Split out so an incremental session can reparse one file and reuse the
/// other 450 files' facts unchanged. Taking facts rather than sources is what
/// makes that possible without duplicating the resolver.
#[tracing::instrument(skip_all, fields(files = sources.len()))]
pub fn build_from_facts(sources: &[(String, String)], facts: &[FileFacts]) -> Index {
    assert_eq!(
        sources.len(),
        facts.len(),
        "facts must line up with sources: file ids are positional"
    );
    let mut prepared = prepare(sources, facts);

    // ---- Call-site arena. ----
    // Recorded before any edge is pushed: `Index::push_edge` reads
    // `sites[..].enclosing` to build the forward adjacency.
    let mut site_scopes: Vec<(usize, Vec<String>)> = Vec::new();
    for (i, f) in facts.iter().enumerate() {
        let file = FileId::from_index(i);
        for c in &f.call_sites {
            let enclosing = enclosing_symbol(&prepared.tables[i], &c.scope);
            prepared.index.sites.push(CallSiteEntry {
                kind: match c.shape {
                    CalleeShape::Name(_) => crate::graph::CalleeKind::Name,
                    CalleeShape::Attribute { .. } => crate::graph::CalleeKind::Attribute,
                    CalleeShape::Dynamic => crate::graph::CalleeKind::Dynamic,
                },
                file,
                span: c.span,
                enclosing,
            });
            site_scopes.push((i, c.scope.clone()));
        }
    }

    // ---- Bind. ----
    // Resolution reads the symbol arena and the interner; `push_edge` writes
    // the adjacency lists. Collecting outcomes first keeps the read phase and
    // the write phase from overlapping, rather than reaching for interior
    // mutability to paper over the conflict.
    let outcomes: Vec<Outcome> = {
        let resolver = prepared.resolver();
        let mut site_id = 0usize;
        let mut outcomes = Vec::with_capacity(prepared.index.sites.len());
        for (i, f) in facts.iter().enumerate() {
            for c in &f.call_sites {
                debug_assert_eq!(site_scopes[site_id].0, i);
                let scope = &site_scopes[site_id].1;
                outcomes.push(resolver.resolve(FileId::from_index(i), scope, &c.shape));
                site_id += 1;
            }
        }
        outcomes
    };

    let mut index = prepared.index;
    let mut coverage = Coverage::default();
    let mut by_shape: std::collections::BTreeMap<String, Coverage> =
        std::collections::BTreeMap::new();
    for (site_id, outcome) in outcomes.into_iter().enumerate() {
        let shape = by_shape
            .entry(index.sites[site_id].kind.as_str().to_string())
            .or_default();
        match outcome {
            // An external call site produces no edge at all. That is what
            // keeps `edges` equal to the recall denominator: every edge is a
            // call site whose callee could be first-party.
            Outcome::External => {
                coverage.record_external();
                shape.record_external();
            }
            Outcome::Edge(target) => {
                match &target {
                    Target::Resolved(_) => {
                        coverage.record_resolved();
                        shape.record_resolved();
                    }
                    Target::Candidates(_) => {
                        coverage.record_candidates();
                        shape.record_candidates();
                    }
                    Target::Unresolved(r) => {
                        coverage.record_unresolved(*r);
                        shape.record_unresolved(*r);
                    }
                }
                index.push_edge(Edge {
                    from: CallSiteId::from_index(site_id),
                    target,
                });
            }
        }
    }

    index.coverage = coverage;
    index.by_shape = by_shape;
    index.fingerprint = crate::cache::fingerprint(sources);
    index
}

/// Everything the resolver reads, built before a single call is bound: the
/// symbol arena, the per-file tables and the class hierarchy.
///
/// Its own step so the Pyright backstop can ask the hierarchy the question
/// ADR 0012 asks (does a first-party subclass override this method?) with
/// the resolver's own rule rather than a second copy of it.
struct Prepared {
    index: Index,
    tables: Vec<FileTables>,
    module_map: HashMap<String, FileId>,
    opaque_decorated: HashSet<SymbolId>,
    class_info: HashMap<SymbolId, ClassInfo>,
    subclasses: HashMap<SymbolId, SmallVec<[SymbolId; 4]>>,
}

impl Prepared {
    fn resolver(&self) -> Resolver<'_> {
        Resolver {
            tables: &self.tables,
            module_map: &self.module_map,
            symbols: &self.index.symbols,
            files: &self.index.files,
            interner: &self.index.interner,
            opaque_decorated: &self.opaque_decorated,
            class_info: &self.class_info,
            subclasses: &self.subclasses,
        }
    }
}

/// The module map, the symbol arena, the definition and import tables, and
/// the class hierarchy with its subclass map. No call site is touched.
fn prepare(sources: &[(String, String)], facts: &[FileFacts]) -> Prepared {
    let mut index = Index::new();
    let mut module_map: HashMap<String, FileId> = HashMap::new();
    let mut first_party_roots: HashSet<String> = HashSet::new();
    // Every dotted prefix of every first-party module, so that "is this a
    // package this repository actually contains" is an O(1) question.
    let mut packages: HashSet<String> = HashSet::new();

    // ---- Pass 2a: the module map. ----
    for (i, (rel, src)) in sources.iter().enumerate() {
        let module = module_path_of(rel);
        if let Some(root) = module.split('.').next()
            && !root.is_empty()
        {
            first_party_roots.insert(root.to_string());
        }
        let segs: Vec<&str> = module.split('.').filter(|s| !s.is_empty()).collect();
        for n in 1..segs.len() {
            packages.insert(segs[..n].join("."));
        }
        // A duplicate module path (the same package vendored twice) keeps the
        // first: `discover` sorts, so "first" is deterministic.
        module_map.entry(module.clone()).or_insert(FileId(i as u32));
        index.files.push(FileEntry {
            path: rel.clone(),
            module,
            hash: content_hash(src),
        });
    }

    // ---- Symbol arena, and the per-file definition tables. ----
    let mut tables: Vec<FileTables> = (0..sources.len()).map(|_| FileTables::default()).collect();
    let mut opaque_decorated: HashSet<SymbolId> = HashSet::new();

    for (i, f) in facts.iter().enumerate() {
        let file = FileId::from_index(i);
        for d in &f.defs {
            let sym = SymbolId::from_index(index.symbols.len());
            index.symbols.push(SymbolEntry {
                name: index.interner.intern(&d.name),
                file,
                span: d.span,
                kind: d.kind,
            });
            tables[i]
                .defs
                .entry(key(&d.scope, &d.name))
                .or_default()
                .push(sym);
            if d.decorators.iter().any(|dec| !is_transparent(dec)) {
                opaque_decorated.insert(sym);
            }
        }
        for b in &f.bindings {
            tables[i].bindings.insert(key(&b.scope, &b.name));
        }
        for t in &f.types {
            let slot = tables[i].types.entry(key(&t.scope, &t.name)).or_default();
            if !slot.contains(&t.ty) {
                slot.push(t.ty.clone());
            }
        }
    }

    // Class hierarchy, keyed by symbol so a method lookup can walk it.
    let mut class_info: HashMap<SymbolId, ClassInfo> = HashMap::new();
    {
        let mut sym = 0usize;
        for (i, f) in facts.iter().enumerate() {
            for d in &f.defs {
                let id = SymbolId::from_index(sym);
                sym += 1;
                if d.kind == SymbolKind::Class {
                    let mut path = d.scope.clone();
                    path.push(d.name.clone());
                    class_info.insert(
                        id,
                        ClassInfo {
                            file: FileId::from_index(i),
                            path,
                            bases: d.bases.clone(),
                        },
                    );
                }
            }
        }
        debug_assert_eq!(
            sym,
            index.symbols.len(),
            "symbol arena walked twice consistently"
        );
    }

    // ---- Pass 2b: import tables. ----
    for (i, f) in facts.iter().enumerate() {
        let pkg = package_of(&index.files[i].module, &index.files[i].path);
        for imp in &f.imports {
            if imp.is_star {
                tables[i].has_star = true;
                continue;
            }
            if imp.names.is_empty() {
                continue;
            }
            // `import a.b.c` carries no `from`, so level is 0 and each name is
            // itself a module path. `from x import a, b` shares one module.
            let is_plain = imp.level == 0 && imp.module == imp.names[0].0;
            for (name, alias) in &imp.names {
                let binding = if is_plain {
                    match lookup_module(&module_map, name) {
                        Some(f) => ImportBinding::ModuleRef(f),
                        None => external_or_unresolvable(name, &first_party_roots, &packages),
                    }
                } else {
                    let target = absolute_module(&imp.module, imp.level, &pkg);
                    match lookup_module(&module_map, &target) {
                        Some(f) => {
                            // `from pkg import sub` may name a submodule
                            // rather than a definition inside `pkg`.
                            let sub = join_module(&target, name);
                            match lookup_module(&module_map, &sub) {
                                Some(sf) => ImportBinding::ModuleRef(sf),
                                None => ImportBinding::FromModule {
                                    file: f,
                                    name: name.clone(),
                                },
                            }
                        }
                        None => {
                            // The module itself did not resolve, but the name
                            // may still be a first-party submodule of a
                            // namespace package.
                            match lookup_module(&module_map, &join_module(&target, name)) {
                                Some(sf) => ImportBinding::ModuleRef(sf),
                                None => {
                                    external_or_unresolvable(&target, &first_party_roots, &packages)
                                }
                            }
                        }
                    }
                };
                tables[i]
                    .imports
                    .entry(alias.clone())
                    .or_default()
                    .push(binding);
            }
        }
    }

    // ---- Subclass map. ----
    // The subclass map needs base-class names resolved, which needs a
    // `Resolver` — so one is built with an empty map purely to compute it.
    // Cheap: a `Resolver` is a handful of references.
    let empty: HashMap<SymbolId, SmallVec<[SymbolId; 4]>> = HashMap::new();
    let mut subclasses: HashMap<SymbolId, SmallVec<[SymbolId; 4]>> = HashMap::new();
    {
        let bootstrap = Resolver {
            tables: &tables,
            module_map: &module_map,
            symbols: &index.symbols,
            files: &index.files,
            interner: &index.interner,
            opaque_decorated: &opaque_decorated,
            class_info: &class_info,
            subclasses: &empty,
        };
        for (&derived, info) in &class_info {
            let outer = &info.path[..info.path.len() - 1];
            for base in &info.bases {
                if let Some(b) = bootstrap.find_class(info.file, outer, base) {
                    subclasses.entry(b).or_default().push(derived);
                }
            }
        }
    }

    Prepared {
        index,
        tables,
        module_map,
        opaque_decorated,
        class_info,
        subclasses,
    }
}

/// The methods of `index` that a first-party subclass overrides, by the rule
/// the resolver applies to a declared receiver type (ADR 0012): the search
/// is rooted at the class that defines the method and walks its whole
/// subtree.
///
/// `sources` and `facts` must be what `index` was built from. Symbol ids are
/// positional, so a set computed from anything else would name the wrong
/// symbols; `None` when the symbols built here differ from `index`'s in
/// count, file, span, kind or name.
pub(crate) fn overridden_methods(
    index: &Index,
    sources: &[(String, String)],
    facts: &[FileFacts],
) -> Option<HashSet<SymbolId>> {
    if sources.len() != facts.len() || sources.len() != index.files.len() {
        return None;
    }
    let prepared = prepare(sources, facts);
    let ours = &prepared.index;
    if ours.symbols.len() != index.symbols.len() {
        return None;
    }
    let same = ours.symbols.iter().zip(&index.symbols).all(|(a, b)| {
        a.file == b.file
            && a.span == b.span
            && a.kind == b.kind
            && ours.interner.resolve(a.name) == index.interner.resolve(b.name)
    });
    if !same {
        return None;
    }

    // A class defined twice at one scope (`if TYPE_CHECKING:` branches)
    // owns the method under either definition, so both are searched.
    let mut classes_at: HashMap<(FileId, &[String]), SmallVec<[SymbolId; 2]>> = HashMap::new();
    for (&class, info) in &prepared.class_info {
        classes_at
            .entry((info.file, info.path.as_slice()))
            .or_default()
            .push(class);
    }
    let resolver = prepared.resolver();
    let mut out = HashSet::new();
    let mut sym = 0usize;
    for (i, f) in facts.iter().enumerate() {
        for d in &f.defs {
            let id = SymbolId::from_index(sym);
            sym += 1;
            if d.kind != SymbolKind::Method {
                continue;
            }
            let owners = classes_at.get(&(FileId::from_index(i), d.scope.as_slice()));
            if owners.is_some_and(|cs| {
                cs.iter()
                    .any(|&c| !resolver.overriding_subclasses(c, &d.name).is_empty())
            }) {
                out.insert(id);
            }
        }
    }
    Some(out)
}

/// Whether a decorator leaves the definition reachable by a plain call.
fn is_transparent(decorator: &str) -> bool {
    // `functools.lru_cache(maxsize=None)` -> `lru_cache`.
    let head = decorator.split('(').next().unwrap_or(decorator).trim();
    let last = head.rsplit('.').next().unwrap_or(head).trim();
    TRANSPARENT_DECORATORS.contains(&last)
}

pub(crate) fn content_hash(src: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    h.finish()
}

/// The package a module lives in. A package's `__init__.py` *is* its package.
fn package_of(module: &str, path: &str) -> String {
    if path.ends_with("__init__.py") {
        return module.to_string();
    }
    match module.rfind('.') {
        Some(i) => module[..i].to_string(),
        None => String::new(),
    }
}

/// Turns a possibly-relative import into an absolute module path.
///
/// Level 1 is the current package, level 2 its parent, and so on.
///
/// Public so the precision audit (`examples/check_reachability.rs`) resolves
/// imports the way the resolver does. An audit that reimplemented this would
/// measure the gap between two implementations rather than the resolver's
/// soundness, and would report its own bugs as unsound edges.
pub fn absolute_module(module: &str, level: u32, pkg: &str) -> String {
    if level == 0 {
        return module.to_string();
    }
    let mut base: Vec<&str> = if pkg.is_empty() {
        Vec::new()
    } else {
        pkg.split('.').collect()
    };
    for _ in 1..level {
        if base.pop().is_none() {
            break;
        }
    }
    let base = base.join(".");
    join_module(&base, module)
}

fn join_module(base: &str, rest: &str) -> String {
    match (base.is_empty(), rest.is_empty()) {
        (true, _) => rest.to_string(),
        (_, true) => base.to_string(),
        _ => format!("{base}.{rest}"),
    }
}

fn lookup_module(map: &HashMap<String, FileId>, module: &str) -> Option<FileId> {
    if module.is_empty() {
        return None;
    }
    map.get(module).copied()
}

/// Classifies a module path that did not resolve to a file in this repository.
///
/// Sharing a root package with first-party code does not make a module
/// first-party. `poetry` imports `poetry.core.packages.package` 379 times, and
/// `poetry-core` is a separate distribution that is not in this repository at
/// all — `src/poetry/core/` does not exist. Judging by the root alone puts
/// those 379 imports in the recall denominator as misses, which is precisely
/// the denominator error `BENCHMARK_AND_EVAL.md` §2 warns about: it understates
/// recall by the third-party share.
///
/// So a module counts as first-party only when this repository actually
/// contains something beneath it (`packages` holds every dotted prefix of
/// every first-party module). Anything else shares a namespace and nothing
/// more.
fn external_or_unresolvable(
    module: &str,
    roots: &HashSet<String>,
    packages: &HashSet<String>,
) -> ImportBinding {
    let root = module.split('.').next().unwrap_or(module);
    if !roots.contains(root) {
        return ImportBinding::External;
    }
    if packages.contains(module) {
        // A real first-party package whose module we failed to bind. That is
        // our gap, so it stays in the denominator.
        ImportBinding::Unresolvable
    } else {
        ImportBinding::External
    }
}

/// The definition whose body contains a call site with this scope chain.
fn enclosing_symbol(t: &FileTables, scope: &[String]) -> Option<SymbolId> {
    let (name, outer) = scope.split_last()?;
    let syms = t.defs.get(&key(outer, name))?;
    // A duplicated definition at one scope (`if TYPE_CHECKING:` branches)
    // makes the owner ambiguous; the first is stable and good enough for
    // attribution, which is not a correctness claim.
    syms.first().copied()
}

/// A call site's outcome, including the fourth case `Target` deliberately
/// does not model: not first-party at all.
enum Outcome {
    Edge(Target),
    External,
}

struct Resolver<'a> {
    tables: &'a [FileTables],
    module_map: &'a HashMap<String, FileId>,
    symbols: &'a [SymbolEntry],
    files: &'a [FileEntry],
    interner: &'a trellis_core::Interner,
    opaque_decorated: &'a HashSet<SymbolId>,
    class_info: &'a HashMap<SymbolId, ClassInfo>,
    /// base class -> its direct first-party subclasses.
    ///
    /// A declared type is an upper bound, not an identity: `repo: Repository`
    /// may hold an `InstalledRepository` at runtime, and if that subclass
    /// overrides the method then resolving to the base's definition is an
    /// unsound edge. The audit caught exactly this, twice. See
    /// `overriding_subclasses`.
    subclasses: &'a HashMap<SymbolId, SmallVec<[SymbolId; 4]>>,
}

impl Resolver<'_> {
    fn resolve(&self, file: FileId, scope: &[String], shape: &CalleeShape) -> Outcome {
        match shape {
            CalleeShape::Name(n) => self.resolve_name(file, scope, n),
            CalleeShape::Attribute { base, attr } => {
                self.resolve_attribute(file, scope, base, attr)
            }
            CalleeShape::Dynamic => Outcome::Edge(Target::Unresolved(Reason::DynamicAttribute)),
        }
    }

    /// `foo()` — walk the scope chain outward, then the import table.
    fn resolve_name(&self, file: FileId, scope: &[String], name: &str) -> Outcome {
        let t = &self.tables[file.index()];

        // Innermost scope first. `i == scope.len()` is a closure defined in
        // the current function; `i == 0` is module level.
        for i in (0..=scope.len()).rev() {
            let k = key(&scope[..i], name);
            let defs = t.defs.get(&k);
            let shadowed = t.bindings.contains(&k);

            match (defs, shadowed) {
                // A definition and a rebinding of the same name at the same
                // scope. Which one is live depends on execution order, which
                // is exactly what static analysis cannot see.
                (Some(_), true) => {
                    return Outcome::Edge(Target::Unresolved(Reason::ShadowedAmbiguous));
                }
                (Some(d), false) => {
                    // At module scope a name can be bound by a definition
                    // *and* by an import — the
                    // `try: from _speedups import f / except ImportError: def f()`
                    // pattern. Taking the definition and ignoring the import
                    // would name one of two real bindings and call it certain.
                    let from_import = if i == 0 {
                        self.import_symbols(t, name)
                    } else {
                        SmallVec::new()
                    };
                    if !from_import.is_empty() {
                        let mut all: SmallVec<[SymbolId; 4]> = d.iter().copied().collect();
                        all.extend(from_import);
                        return Outcome::Edge(Target::Candidates(all));
                    }
                    if d.len() == 1 {
                        return self.emit(d[0]);
                    }
                    return Outcome::Edge(Target::Candidates(d.iter().copied().collect()));
                }
                // A parameter or local. The true callee is whatever was
                // passed in — frequently first-party, so this is a miss.
                (None, true) => {
                    return Outcome::Edge(Target::Unresolved(Reason::LocalBinding));
                }
                (None, false) => {}
            }
        }

        if let Some(binds) = t.imports.get(name) {
            return self.outcome_of_bindings(binds);
        }
        if BUILTINS.contains(&name) {
            return Outcome::External;
        }
        if t.has_star {
            return Outcome::Edge(Target::Unresolved(Reason::StarImport));
        }
        Outcome::Edge(Target::Unresolved(Reason::UnboundName))
    }

    /// The single first-party class a type expression names, if exactly one.
    ///
    /// Returns `None` on anything ambiguous. A type fact that resolves to a
    /// *function* also returns `None`, which is what makes
    /// `x = make_thing()` safe to record in Pass 1: the fact exists but
    /// resolves to nothing, so it types nothing.
    fn find_class(&self, file: FileId, scope: &[String], ty: &str) -> Option<SymbolId> {
        let t = &self.tables[file.index()];
        let is_class = |s: &SymbolId| self.symbols[s.index()].kind == SymbolKind::Class;

        // `models.Foo` — qualified by a module alias.
        if let Some((prefix, last)) = ty.rsplit_once('.') {
            let binds = t.imports.get(prefix)?;
            let mut found = None;
            for b in binds {
                if let Bound::Module(f) = self.follow(b, MAX_REEXPORT_DEPTH)
                    && let Bound::Symbol(s) =
                        self.lookup_in_module(f, last, MAX_REEXPORT_DEPTH, &mut HashSet::new())
                    && is_class(&s)
                {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(s);
                }
            }
            return found;
        }

        for i in (0..=scope.len()).rev() {
            if let Some(d) = t.defs.get(&key(&scope[..i], ty)) {
                let classes: SmallVec<[SymbolId; 2]> = d.iter().copied().filter(is_class).collect();
                return (classes.len() == 1).then(|| classes[0]);
            }
        }

        let binds = t.imports.get(ty)?;
        let mut found = None;
        for b in binds {
            if let Bound::Symbol(s) = self.follow(b, MAX_REEXPORT_DEPTH)
                && is_class(&s)
            {
                if found.is_some() {
                    return None;
                }
                found = Some(s);
            }
        }
        found
    }

    /// Whether a stated type can be *shown* not to be first-party.
    ///
    /// Only two ways count, and both are positive evidence rather than a
    /// failure to find something:
    ///
    /// - the name is a Python builtin and nothing first-party shadows it;
    /// - every binding for the name resolves outside the repository root.
    ///
    /// Everything else is `false`, which keeps the site in the denominator as
    /// a miss. That asymmetry is deliberate: this function decides what leaves
    /// the recall denominator, so a permissive answer here inflates recall
    /// invisibly, and the inflation would be indistinguishable from real
    /// improvement.
    fn type_is_demonstrably_external(&self, file: FileId, scope: &[String], ty: &str) -> bool {
        let t = &self.tables[file.index()];

        // `pathlib.Path` — judge the module alias, which is what determines
        // where the name comes from.
        let (lookup, is_qualified) = match ty.rsplit_once('.') {
            Some((prefix, _)) => (prefix, true),
            None => (ty, false),
        };

        // A first-party definition of the name anywhere in scope settles it:
        // whatever else is true, this is not demonstrably external. Checked
        // before the builtin list so a repository that defines `class Path`
        // or shadows a builtin is not mis-excluded.
        for i in (0..=scope.len()).rev() {
            if t.defs.contains_key(&key(&scope[..i], lookup)) {
                return false;
            }
        }

        match t.imports.get(lookup) {
            Some(binds) if !binds.is_empty() => binds
                .iter()
                .all(|b| matches!(self.follow(b, MAX_REEXPORT_DEPTH), Bound::External)),
            // No import and no definition. A builtin is the only way a bare
            // name is legitimately in scope, and a qualified name whose
            // prefix is bound to nothing tells us nothing at all.
            _ => !is_qualified && BUILTIN_TYPES.contains(&ty),
        }
    }

    /// The type stated for a name in a scope, if the source states exactly one.
    fn stated_type(&self, file: FileId, scope: &[String], name: &str) -> Option<String> {
        let t = &self.tables[file.index()];
        for i in (0..=scope.len()).rev() {
            if let Some(tys) = t.types.get(&key(&scope[..i], name)) {
                // Conflicting annotations mean the source disagrees with
                // itself; declining is the only sound reading.
                return (tys.len() == 1).then(|| tys[0].clone());
            }
        }
        None
    }

    /// Finds `attr` on a class, then on its bases.
    ///
    /// Python's MRO is a linearization we do not model, so finding the name on
    /// two unrelated bases yields `Many` rather than a choice.
    fn lookup_method(
        &self,
        class: SymbolId,
        attr: &str,
        depth: u32,
        visited: &mut HashSet<SymbolId>,
    ) -> MethodLookup {
        if !visited.insert(class) || depth == 0 {
            return MethodLookup::NotFound { exhaustive: false };
        }
        let Some(info) = self.class_info.get(&class) else {
            return MethodLookup::NotFound { exhaustive: false };
        };
        let t = &self.tables[info.file.index()];

        if let Some(d) = t.defs.get(&key(&info.path, attr)) {
            // Q-14. A class body may both `def run(self)` and assign `run`,
            // and whichever comes last wins — `run = staticmethod(run)`,
            // `run = property(run)`, or a plain rebinding. This path checked
            // `defs` and not `bindings`, so it emitted a confident edge to the
            // `def` whatever the assignment did to it.
            //
            // That is an unsound edge, which the v1.0 gate forbids outright,
            // and the `Name` path already reports the equivalent case as
            // `ShadowedAmbiguous`. Making the two agree is the fix; the cost
            // is recall on a pattern that did not appear anywhere in the
            // audited sample.
            if t.bindings.contains(&key(&info.path, attr)) {
                return MethodLookup::Shadowed;
            }
            return match d.len() {
                1 => MethodLookup::One {
                    method: d[0],
                    owner: class,
                },
                _ => MethodLookup::Many(d.iter().copied().collect()),
            };
        }

        // Not on this class. Walk the bases, tracking whether we actually
        // managed to search all of them.
        let mut found: SmallVec<[SymbolId; 4]> = SmallVec::new();
        let mut owners: SmallVec<[SymbolId; 4]> = SmallVec::new();
        let mut exhaustive = true;
        // A base is named in the scope enclosing the class, not inside it.
        let outer = &info.path[..info.path.len() - 1];
        for base in &info.bases {
            match self.find_class(info.file, outer, base) {
                Some(b) => match self.lookup_method(b, attr, depth - 1, visited) {
                    MethodLookup::One { method, owner } => {
                        found.push(method);
                        owners.push(owner);
                    }
                    MethodLookup::Many(v) => found.extend(v),
                    // A base whose method is shadowed by a class-scope
                    // assignment makes the whole lookup ambiguous: inheriting
                    // from it inherits whatever the assignment produced, which
                    // we cannot name. Propagating rather than treating it as a
                    // miss keeps the reason honest.
                    MethodLookup::Shadowed => return MethodLookup::Shadowed,
                    MethodLookup::NotFound { exhaustive: e } => exhaustive &= e,
                },
                // A third-party or unresolvable base. The method may well be
                // there, so absence here proves nothing.
                None => exhaustive = false,
            }
        }
        match found.len() {
            0 => MethodLookup::NotFound { exhaustive },
            1 => MethodLookup::One {
                method: found[0],
                owner: owners[0],
            },
            _ => MethodLookup::Many(found),
        }
    }

    /// Turns a method lookup into a call outcome.
    fn method_outcome(&self, lookup: MethodLookup) -> Outcome {
        match lookup {
            MethodLookup::One { method, .. } => self.emit(method),
            MethodLookup::Many(v) => Outcome::Edge(Target::Candidates(v)),
            MethodLookup::Shadowed => Outcome::Edge(Target::Unresolved(Reason::ShadowedAmbiguous)),
            // Even an exhaustive miss stays `MethodDispatchUntyped` rather
            // than becoming `External`: the attribute may be set dynamically,
            // and claiming it is third-party would shrink the resolver's own
            // denominator on a guess (A-31).
            MethodLookup::NotFound { .. } => {
                Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped))
            }
        }
    }

    /// Subclasses of `class` that define their own `attr`.
    ///
    /// Walks the whole subtree, not just direct children: an override two
    /// levels down is just as capable of receiving the call.
    fn overriding_subclasses(&self, class: SymbolId, attr: &str) -> SmallVec<[SymbolId; 4]> {
        let mut out: SmallVec<[SymbolId; 4]> = SmallVec::new();
        let mut seen: HashSet<SymbolId> = HashSet::from([class]);
        let mut queue = std::collections::VecDeque::from([class]);
        while let Some(c) = queue.pop_front() {
            for &sub in self.subclasses.get(&c).map_or(&[][..], |v| v.as_slice()) {
                if !seen.insert(sub) {
                    continue;
                }
                queue.push_back(sub);
                if let Some(info) = self.class_info.get(&sub)
                    && let Some(d) = self.tables[info.file.index()]
                        .defs
                        .get(&key(&info.path, attr))
                {
                    out.extend(d.iter().copied());
                }
            }
        }
        out
    }

    /// `obj.attr()` where `obj`'s type is stated somewhere in scope.
    ///
    /// The stated type is an **upper bound**. Python dispatches on the runtime
    /// type, so if any first-party subclass overrides `attr`, the call may
    /// reach that override instead and the base's definition is not a fact.
    /// All of them are recorded and none is chosen — which is what
    /// `Candidates` is for (ADR 0002).
    ///
    /// This is deliberately *not* applied to `self.m()`: the audit protocol
    /// (`BENCHMARK_AND_EVAL.md` §2, rule 1) resolves those to the enclosing
    /// class's definition, treating subclass override as the dynamic dispatch
    /// Constraint 1 accepts as unknowable. The asymmetry is real — a declared
    /// parameter type is a weaker claim than a lexically enclosing class.
    fn typed_receiver(
        &self,
        file: FileId,
        scope: &[String],
        receiver: &str,
        attr: &str,
    ) -> Option<Outcome> {
        let ty = self.stated_type(file, scope, receiver)?;
        let Some(class) = self.find_class(file, scope, &ty) else {
            // The source states a type and it is not a first-party class.
            //
            // If we can *show* it is not — a builtin, or a name every binding
            // of which resolves outside the repository root — the call is not
            // a first-party call we missed. It is a call into the standard
            // library or a dependency, and BENCHMARK_AND_EVAL.md §2 defines
            // recall over call sites whose true callee is first-party. So it
            // is excluded, exactly like the imports already excluded there.
            //
            // `val: str` calling `val.lower()` was previously recorded as
            // MethodDispatchUntyped and counted against recall, which is the
            // same defect `poetry.core` produced: 379 imports in the
            // denominator as first-party misses.
            //
            // "Demonstrably" is the whole discipline. A type the resolver
            // merely fails to resolve stays a miss. Excluding those instead
            // would turn every failure into an exclusion and make recall
            // unfalsifiable — a number that can only go up is not a
            // measurement.
            return self
                .type_is_demonstrably_external(file, scope, &ty)
                .then_some(Outcome::External);
        };
        let found = self.lookup_method(class, attr, MAX_INHERITANCE_DEPTH, &mut HashSet::new());

        // Search for overrides from the class the definition was FOUND on,
        // not from the declared type. An annotation is evidence, not proof —
        // the audit found `installed: DummyRepository` bound at runtime to an
        // `InstalledRepository`, a *sibling* that overrides the method. Both
        // descend from the class that defines it, so rooting the search there
        // catches the sibling; rooting it at the declared type does not.
        let root = match &found {
            MethodLookup::One { owner, .. } => *owner,
            _ => class,
        };
        // A shadowed method has no owner to root an override search at, and
        // merging it into a candidate set would put a symbol in the radius
        // that the assignment may have replaced outright.
        if matches!(found, MethodLookup::Shadowed) {
            return Some(Outcome::Edge(Target::Unresolved(Reason::ShadowedAmbiguous)));
        }

        let overrides = self.overriding_subclasses(root, attr);
        if overrides.is_empty() {
            return Some(self.method_outcome(found));
        }
        let mut all: SmallVec<[SymbolId; 4]> = match found {
            MethodLookup::One { method, .. } => SmallVec::from_slice(&[method]),
            MethodLookup::Many(v) => v,
            // Returned above; the arm stays so a future variant cannot be
            // absorbed silently.
            MethodLookup::Shadowed | MethodLookup::NotFound { .. } => SmallVec::new(),
        };
        all.extend(overrides);
        all.sort_unstable();
        all.dedup();
        Some(match all.len() {
            0 => Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped)),
            1 => self.emit(all[0]),
            _ => Outcome::Edge(Target::Candidates(all)),
        })
    }

    /// First-party symbols an import alias binds, if any.
    ///
    /// Used to detect a name bound by both a definition and an import, which
    /// is `Candidates` rather than a choice between them.
    fn import_symbols(&self, t: &FileTables, name: &str) -> SmallVec<[SymbolId; 4]> {
        let mut out = SmallVec::new();
        let Some(binds) = t.imports.get(name) else {
            return out;
        };
        for b in binds {
            match self.follow(b, MAX_REEXPORT_DEPTH) {
                Bound::Symbol(s) => out.push(s),
                Bound::Symbols(v) => out.extend(v),
                Bound::Module(_) | Bound::External | Bound::Unfollowed(_) => {}
            }
        }
        out
    }

    /// `a.b()` — resolvable only when the receiver's origin is known.
    fn resolve_attribute(&self, file: FileId, scope: &[String], base: &str, attr: &str) -> Outcome {
        let t = &self.tables[file.index()];

        // `self.m()` / `cls.m()` inside a method: the enclosing class, then
        // its bases. Inheritance is how the benchmark corpus is written, so
        // stopping at the class itself loses most of these (ADR 0012).
        if base == "self" || base == "cls" {
            return match self.enclosing_class_symbol(file, scope) {
                Some(class) => self.method_outcome(self.lookup_method(
                    class,
                    attr,
                    MAX_INHERITANCE_DEPTH,
                    &mut HashSet::new(),
                )),
                None => Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped)),
            };
        }

        // `self._x.m()` — an instance attribute whose type the source states.
        if let Some(rest) = base.strip_prefix("self.")
            && !rest.contains('.')
            && let Some(class) = self.enclosing_class_symbol(file, scope)
            && let Some(info) = self.class_info.get(&class)
            && let Some(out) =
                self.typed_receiver(info.file, &info.path, &format!("self.{rest}"), attr)
        {
            return out;
        }

        // The whole dotted base as an import alias: `from a import b; b.c()`.
        if let Some(binds) = t.imports.get(base) {
            return self.attr_of_bindings(binds, attr);
        }

        // `import a.b.c` binds `a`; the call reads `a.b.c.d()`, so the base
        // is a module path in its own right.
        if let Some(root) = base.split('.').next()
            && let Some(binds) = t.imports.get(root)
        {
            if let Some(f) = lookup_module(self.module_map, base) {
                return self.attr_of_bound(&Bound::Module(f), attr);
            }
            // The root binds outside the repository, so the whole chain does.
            // `import os.path` then `os.path.join(...)` is not a first-party
            // call site at all, and counting it as a miss would understate
            // recall by the third-party share of calls — which is large.
            if binds.iter().all(|b| matches!(b, ImportBinding::External)) {
                return Outcome::External;
            }
        }

        // A local or parameter whose type the source states: `env.run()`
        // where `env` is annotated, or was assigned from a constructor.
        if !base.contains('.')
            && let Some(out) = self.typed_receiver(file, scope, base, attr)
        {
            return out;
        }

        // A first-party class named in this file: `C.make()`.
        if !base.contains('.') {
            for i in (0..=scope.len()).rev() {
                let k = key(&scope[..i], base);
                if t.bindings.contains(&k) {
                    // An instance whose type is not stated anywhere we can
                    // see — the typed-receiver path above already declined.
                    return Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped));
                }
                if let Some(d) = t.defs.get(&k) {
                    return match d.first() {
                        Some(&s) if self.symbols[s.index()].kind == SymbolKind::Class => {
                            let mut class_path = scope[..i].to_vec();
                            class_path.push(base.to_string());
                            let method_key = key(&class_path, attr);
                            match t.defs.get(&method_key) {
                                // Q-14, as in `lookup_method`: a class-scope
                                // assignment may have replaced the `def`.
                                Some(_) if t.bindings.contains(&method_key) => {
                                    Outcome::Edge(Target::Unresolved(Reason::ShadowedAmbiguous))
                                }
                                Some(m) if m.len() == 1 => self.emit(m[0]),
                                Some(m) => {
                                    Outcome::Edge(Target::Candidates(m.iter().copied().collect()))
                                }
                                None => {
                                    Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped))
                                }
                            }
                        }
                        // An attribute of a function object.
                        _ => Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped)),
                    };
                }
            }
        }

        if BUILTINS.contains(&base) {
            return Outcome::External;
        }
        if t.has_star {
            return Outcome::Edge(Target::Unresolved(Reason::StarImport));
        }
        Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped))
    }

    /// The innermost enclosing `class` in a scope chain, as a symbol.
    ///
    /// Returns the symbol rather than the name because every consumer needs
    /// the class's bases and defining file, which the name alone cannot give.
    fn enclosing_class_symbol(&self, file: FileId, scope: &[String]) -> Option<SymbolId> {
        let t = &self.tables[file.index()];
        for i in (0..scope.len()).rev() {
            let k = key(&scope[..i], &scope[i]);
            if let Some(d) = t.defs.get(&k) {
                let classes: SmallVec<[SymbolId; 2]> = d
                    .iter()
                    .copied()
                    .filter(|&s| self.symbols[s.index()].kind == SymbolKind::Class)
                    .collect();
                if classes.len() == 1 {
                    return Some(classes[0]);
                }
                if !classes.is_empty() {
                    // Two classes of one name at one scope: which one encloses
                    // this call is not decidable from the scope chain.
                    return None;
                }
            }
        }
        None
    }

    /// Turns a set of import bindings into a call outcome.
    fn outcome_of_bindings(&self, binds: &[ImportBinding]) -> Outcome {
        let bounds: Vec<Bound> = binds
            .iter()
            .map(|b| self.follow(b, MAX_REEXPORT_DEPTH))
            .collect();
        self.collapse(&bounds)
    }

    fn attr_of_bindings(&self, binds: &[ImportBinding], attr: &str) -> Outcome {
        let bounds: Vec<Bound> = binds
            .iter()
            .map(|b| self.follow(b, MAX_REEXPORT_DEPTH))
            .collect();
        let outs: Vec<Outcome> = bounds.iter().map(|b| self.attr_of_bound(b, attr)).collect();
        merge_outcomes(outs)
    }

    /// `x.attr()` where `x` is a resolved import binding.
    fn attr_of_bound(&self, b: &Bound, attr: &str) -> Outcome {
        match b {
            Bound::Module(f) => {
                let bound =
                    self.lookup_in_module(*f, attr, MAX_REEXPORT_DEPTH, &mut HashSet::new());
                self.collapse(std::slice::from_ref(&bound))
            }
            Bound::Symbol(s) if self.symbols[s.index()].kind == SymbolKind::Class => {
                let sym = &self.symbols[s.index()];
                let t = &self.tables[sym.file.index()];
                let class_name = self.interned_name(*s);
                // A class imported from elsewhere is defined at module scope
                // in its own file.
                let method_key = key(&[class_name], attr);
                match t.defs.get(&method_key) {
                    // Q-14, as in `lookup_method`: a class-scope assignment
                    // may have replaced the `def`.
                    Some(_) if t.bindings.contains(&method_key) => {
                        Outcome::Edge(Target::Unresolved(Reason::ShadowedAmbiguous))
                    }
                    Some(m) if m.len() == 1 => self.emit(m[0]),
                    Some(m) => Outcome::Edge(Target::Candidates(m.iter().copied().collect())),
                    None => Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped)),
                }
            }
            Bound::Symbol(_) | Bound::Symbols(_) => {
                Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped))
            }
            Bound::External => Outcome::External,
            Bound::Unfollowed(r) => Outcome::Edge(Target::Unresolved(*r)),
        }
    }

    /// `SymbolEntry.name` is interned; a scope key needs the text back.
    fn interned_name(&self, s: SymbolId) -> String {
        self.interner
            .resolve(self.symbols[s.index()].name)
            .to_string()
    }

    /// Follows an import binding to something nameable.
    fn follow(&self, b: &ImportBinding, depth: u32) -> Bound {
        match b {
            ImportBinding::ModuleRef(f) => Bound::Module(*f),
            ImportBinding::External => Bound::External,
            ImportBinding::Unresolvable => Bound::Unfollowed(Reason::ReexportUnfollowed),
            ImportBinding::FromModule { file, name } => {
                self.lookup_in_module(*file, name, depth, &mut HashSet::new())
            }
        }
    }

    /// Looks a name up inside a first-party module, following re-exports.
    ///
    /// Each hop is a real import statement, so following is sound; the depth
    /// limit is about cycles and cost, not correctness.
    fn lookup_in_module(
        &self,
        file: FileId,
        name: &str,
        depth: u32,
        visited: &mut HashSet<(u32, String)>,
    ) -> Bound {
        if !visited.insert((file.0, name.to_string())) {
            return Bound::Unfollowed(Reason::ReexportUnfollowed);
        }
        let t = &self.tables[file.index()];

        if let Some(d) = t.defs.get(&key(&[], name)) {
            return if d.len() == 1 {
                Bound::Symbol(d[0])
            } else {
                Bound::Symbols(d.iter().copied().collect())
            };
        }
        // A submodule of this package: `from poetry import console`.
        if let Some(f) = lookup_module(self.module_map, &join_module(&self.module_of(file), name)) {
            return Bound::Module(f);
        }
        if let Some(binds) = t.imports.get(name) {
            if depth == 0 {
                return Bound::Unfollowed(Reason::ReexportUnfollowed);
            }
            let bounds: Vec<Bound> = binds
                .iter()
                .map(|b| match b {
                    ImportBinding::FromModule { file: f2, name: n2 } => {
                        self.lookup_in_module(*f2, n2, depth - 1, visited)
                    }
                    other => self.follow(other, depth - 1),
                })
                .collect();
            return merge_bounds(bounds);
        }
        if t.bindings.contains(&key(&[], name)) {
            // A module-level assignment: `handler = _make_handler()`.
            return Bound::Unfollowed(Reason::LocalBinding);
        }
        if t.has_star {
            return Bound::Unfollowed(Reason::StarImport);
        }
        Bound::Unfollowed(Reason::ReexportUnfollowed)
    }

    fn module_of(&self, file: FileId) -> String {
        self.files[file.index()].module.clone()
    }

    /// Applies the decorator rule to a single resolved definition.
    fn emit(&self, s: SymbolId) -> Outcome {
        if self.opaque_decorated.contains(&s) {
            Outcome::Edge(Target::Unresolved(Reason::DecoratorIndirection))
        } else {
            Outcome::Edge(Target::Resolved(s))
        }
    }

    fn collapse(&self, bounds: &[Bound]) -> Outcome {
        let outs: Vec<Outcome> = bounds
            .iter()
            .map(|b| match b {
                Bound::Symbol(s) => self.emit(*s),
                Bound::Symbols(v) => Outcome::Edge(Target::Candidates(v.clone())),
                Bound::External => Outcome::External,
                Bound::Unfollowed(r) => Outcome::Edge(Target::Unresolved(*r)),
                // Calling a module is not valid Python; the binding is wrong.
                Bound::Module(_) => Outcome::Edge(Target::Unresolved(Reason::UnboundName)),
            })
            .collect();
        merge_outcomes(outs)
    }
}

fn merge_bounds(bounds: Vec<Bound>) -> Bound {
    let mut syms: SmallVec<[SymbolId; 4]> = SmallVec::new();
    let mut reason = None;
    let mut external = false;
    for b in bounds {
        match b {
            Bound::Symbol(s) => syms.push(s),
            Bound::Symbols(v) => syms.extend(v),
            Bound::Module(f) => return Bound::Module(f),
            Bound::External => external = true,
            Bound::Unfollowed(r) => reason = Some(r),
        }
    }
    match (syms.len(), reason, external) {
        (1, None, false) => Bound::Symbol(syms[0]),
        (0, Some(r), _) => Bound::Unfollowed(r),
        (0, None, true) => Bound::External,
        (0, None, false) => Bound::Unfollowed(Reason::ReexportUnfollowed),
        _ => Bound::Symbols(syms),
    }
}

/// Folds several outcomes for one call site into one.
///
/// A conditional import binds the same name twice; both are real and neither
/// is chosen. Mixing a first-party binding with an external one still yields
/// `Candidates` over the first-party ones, because a call that *might* reach
/// first-party code belongs in the denominator.
fn merge_outcomes(outs: Vec<Outcome>) -> Outcome {
    if outs.len() == 1 {
        return outs.into_iter().next().expect("length checked");
    }
    let mut syms: SmallVec<[SymbolId; 4]> = SmallVec::new();
    let mut reason = None;
    let mut external = false;
    for o in outs {
        match o {
            Outcome::External => external = true,
            Outcome::Edge(Target::Resolved(s)) => syms.push(s),
            Outcome::Edge(Target::Candidates(v)) => syms.extend(v),
            Outcome::Edge(Target::Unresolved(r)) => reason = Some(r),
        }
    }
    match (syms.len(), reason, external) {
        (0, Some(r), _) => Outcome::Edge(Target::Unresolved(r)),
        (0, None, true) => Outcome::External,
        (0, None, false) => Outcome::Edge(Target::Unresolved(Reason::UnboundName)),
        // Exactly one first-party binding but another branch was external or
        // unresolved: still ambiguous, never `Resolved`.
        (1, None, false) => Outcome::Edge(Target::Resolved(syms[0])),
        _ => Outcome::Edge(Target::Candidates(syms)),
    }
}
