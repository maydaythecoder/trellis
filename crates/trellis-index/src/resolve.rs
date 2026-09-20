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
    "contextmanager",
    "asynccontextmanager",
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
/// ASSUMPTION A-24: `src/` is the only synthetic package root recognised.
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
    let paths = discover(root);
    let sources: Vec<(String, String)> = paths
        .iter()
        .filter_map(|p| {
            let src = std::fs::read_to_string(p).ok()?;
            Some((rel_path(root, p), src))
        })
        .collect();
    build_from_sources(&sources)
}

/// The resolver proper, over `(relative_path, source)` pairs.
///
/// Taking sources rather than a directory is what lets the whole resolver be
/// tested from fixtures with no filesystem layout to arrange, and keeps
/// `trellis-index` free of any dependency beyond `trellis-core`.
#[tracing::instrument(skip_all, fields(files = sources.len()))]
pub fn build_from_sources(sources: &[(String, String)]) -> Index {
    use rayon::prelude::*;

    // ---- Pass 1: parse and collect, in parallel. ----
    let facts: Vec<FileFacts> = sources
        .par_iter()
        .enumerate()
        .map(|(i, (_, src))| crate::parse::collect(src, FileId::from_index(i)))
        .collect();

    let mut index = Index::new();
    let mut module_map: HashMap<String, FileId> = HashMap::new();
    let mut first_party_roots: HashSet<String> = HashSet::new();

    // ---- Pass 2a: the module map. ----
    for (i, (rel, src)) in sources.iter().enumerate() {
        let module = module_path_of(rel);
        if let Some(root) = module.split('.').next()
            && !root.is_empty()
        {
            first_party_roots.insert(root.to_string());
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
                        None => external_or_unresolvable(name, &first_party_roots),
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
                                None => external_or_unresolvable(&target, &first_party_roots),
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

    // ---- Call-site arena. ----
    // Recorded before any edge is pushed: `Index::push_edge` reads
    // `sites[..].enclosing` to build the forward adjacency.
    let mut site_scopes: Vec<(usize, Vec<String>)> = Vec::new();
    for (i, f) in facts.iter().enumerate() {
        let file = FileId::from_index(i);
        for c in &f.call_sites {
            let enclosing = enclosing_symbol(&tables[i], &c.scope);
            index.sites.push(CallSiteEntry {
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
        let resolver = Resolver {
            tables: &tables,
            module_map: &module_map,
            symbols: &index.symbols,
            files: &index.files,
            interner: &index.interner,
            opaque_decorated: &opaque_decorated,
        };
        let mut site_id = 0usize;
        let mut outcomes = Vec::with_capacity(index.sites.len());
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

    let shapes: Vec<&'static str> = facts
        .iter()
        .flat_map(|f| f.call_sites.iter())
        .map(|c| match c.shape {
            CalleeShape::Name(_) => "Name",
            CalleeShape::Attribute { .. } => "Attribute",
            CalleeShape::Dynamic => "Dynamic",
        })
        .collect();

    let mut coverage = Coverage::default();
    let mut by_shape: std::collections::BTreeMap<String, Coverage> =
        std::collections::BTreeMap::new();
    for (site_id, outcome) in outcomes.into_iter().enumerate() {
        let shape = by_shape.entry(shapes[site_id].to_string()).or_default();
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
    index
}

/// Whether a decorator leaves the definition reachable by a plain call.
fn is_transparent(decorator: &str) -> bool {
    // `functools.lru_cache(maxsize=None)` -> `lru_cache`.
    let head = decorator.split('(').next().unwrap_or(decorator).trim();
    let last = head.rsplit('.').next().unwrap_or(head).trim();
    TRANSPARENT_DECORATORS.contains(&last)
}

fn content_hash(src: &str) -> u64 {
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
fn absolute_module(module: &str, level: u32, pkg: &str) -> String {
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

/// A module that did not resolve to a file is external when its root package
/// is not first-party, and a miss when it is.
fn external_or_unresolvable(module: &str, roots: &HashSet<String>) -> ImportBinding {
    let root = module.split('.').next().unwrap_or(module);
    if roots.contains(root) {
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
                (Some(d), false) if d.len() == 1 => return self.emit(d[0]),
                (Some(d), false) => {
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

    /// `a.b()` — resolvable only when the receiver's origin is known.
    fn resolve_attribute(&self, file: FileId, scope: &[String], base: &str, attr: &str) -> Outcome {
        let t = &self.tables[file.index()];

        // `self.m()` / `cls.m()` inside a method: look in the enclosing class.
        if base == "self" || base == "cls" {
            return match self.enclosing_class(file, scope) {
                Some(class) => {
                    let k = key(&class_scope(scope, &class), attr);
                    match t.defs.get(&k) {
                        Some(d) if d.len() == 1 => self.emit(d[0]),
                        Some(d) => Outcome::Edge(Target::Candidates(d.iter().copied().collect())),
                        // Inherited, mixed in, or set dynamically. Following
                        // base classes across files is the obvious recall win
                        // and is not attempted here; the Reason distribution
                        // is what decides whether it is worth building (S1).
                        None => Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped)),
                    }
                }
                None => Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped)),
            };
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

        // A first-party class named in this file: `C.make()`.
        if !base.contains('.') {
            for i in (0..=scope.len()).rev() {
                let k = key(&scope[..i], base);
                if t.bindings.contains(&k) {
                    // An instance, not the class itself. Its type is unknown.
                    return Outcome::Edge(Target::Unresolved(Reason::MethodDispatchUntyped));
                }
                if let Some(d) = t.defs.get(&k) {
                    return match d.first() {
                        Some(&s) if self.symbols[s.index()].kind == SymbolKind::Class => {
                            let mut class_path = scope[..i].to_vec();
                            class_path.push(base.to_string());
                            match t.defs.get(&key(&class_path, attr)) {
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

    /// The innermost `class` in a scope chain, if any.
    fn enclosing_class(&self, file: FileId, scope: &[String]) -> Option<String> {
        let t = &self.tables[file.index()];
        for i in (0..scope.len()).rev() {
            let k = key(&scope[..i], &scope[i]);
            if let Some(d) = t.defs.get(&k)
                && d.iter()
                    .any(|&s| self.symbols[s.index()].kind == SymbolKind::Class)
            {
                return Some(scope[i].clone());
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
                match t.defs.get(&key(&[class_name], attr)) {
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

/// The scope path of a class within an enclosing chain.
fn class_scope(scope: &[String], class: &str) -> Vec<String> {
    let mut out = Vec::new();
    for s in scope {
        out.push(s.clone());
        if s == class {
            break;
        }
    }
    out
}
