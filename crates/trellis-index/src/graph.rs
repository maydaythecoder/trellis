//! Graph storage: dense arenas plus bidirectional adjacency.

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use trellis_core::{CallSiteId, FileId, Interner, SymbolId};

/// Why a call site could not be bound to a first-party definition.
///
/// Constraint 1: PyCG, the academic state of the art, reports roughly 99.2%
/// precision at about 69.9% recall. Around three edges in ten are
/// statically invisible in Python, and that is a property of the language,
/// not a gap better engineering closes. So unresolved is a normal outcome
/// with a reason attached, never an error.
///
/// The distribution over these variants is instrumented from day one: it is
/// the input to the v0.1 gate and to the S1 fallback decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Reason {
    /// `obj.foo()` where `obj`'s type is unknown.
    MethodDispatchUntyped,
    /// Callee is wrapped by a decorator. Expected to dominate: the three
    /// candidate benchmark repos carry 708, 814 and 1812 decorator lines
    /// respectively (see BENCHMARK_AND_EVAL.md).
    DecoratorIndirection,
    /// `from x import *`.
    StarImport,
    /// `getattr`, `__getattr__`, dict dispatch.
    DynamicAttribute,
    /// An `__init__.py` re-export chain too deep or too ambiguous to follow.
    ReexportUnfollowed,
    /// A binding shadowed in a way the resolver cannot disambiguate.
    ShadowedAmbiguous,
}

/// What a call site resolved to.
///
/// Three outcomes, never two. Collapsing `Candidates` into either
/// `Resolved` or `Unresolved` is how a tool starts reporting a blast radius
/// that is confidently wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    /// Exactly one first-party definition matches and nothing nearer
    /// shadows it.
    Resolved(SymbolId),
    /// A known finite set. All recorded, none chosen.
    Candidates(SmallVec<[SymbolId; 4]>),
    /// Statically invisible, with a reason.
    Unresolved(Reason),
}

impl Target {
    /// Symbols this target could reach. `Unresolved` reaches none *that we
    /// can name* — which is not the same as reaching none.
    pub fn symbols(&self) -> &[SymbolId] {
        match self {
            Target::Resolved(s) => std::slice::from_ref(s),
            Target::Candidates(c) => c,
            Target::Unresolved(_) => &[],
        }
    }

    /// Whether this target is certain enough to partition on.
    ///
    /// S19: partition safety inherits the graph's recall. A symbol reached
    /// only through a `Candidates` or `Unresolved` edge is *unpartitionable*
    /// — it and its dependents go into one partition, conservatively.
    pub fn is_certain(&self) -> bool {
        matches!(self, Target::Resolved(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: CallSiteId,
    pub target: Target,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    /// Dotted module path, e.g. `poetry.console.commands.add`.
    pub module: String,
    /// Content hash, for deciding what to reparse.
    pub hash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub name: trellis_core::interner::Sym,
    pub file: FileId,
    /// Byte span of the definition.
    pub span: (usize, usize),
    pub kind: SymbolKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallSiteEntry {
    pub file: FileId,
    pub span: (usize, usize),
    /// The symbol whose body contains this call, when there is one.
    pub enclosing: Option<SymbolId>,
}

/// The call graph.
///
/// Owned outright by the orchestrator: no `Arc<RwLock<Index>>`, no shared
/// mutable graph. See `docs/ADRs/0005-loop-shape.md`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub files: Vec<FileEntry>,
    pub symbols: Vec<SymbolEntry>,
    pub sites: Vec<CallSiteEntry>,
    pub edges: Vec<Edge>,
    /// symbol -> indices into `edges` for calls made *from* that symbol.
    forward: Vec<SmallVec<[u32; 4]>>,
    /// symbol -> indices into `edges` for calls that reach that symbol.
    reverse: Vec<SmallVec<[u32; 4]>>,
    pub interner: Interner,
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }

    /// Adds an edge and updates both adjacency lists in one step.
    ///
    /// Building `forward` and `reverse` together is what makes
    /// bidirectionality a property rather than an achievement: there is no
    /// code path that writes one without the other. The v1.0 gate's
    /// bidirectionality condition is then a property test over the pair
    /// (`tests/bidirectional.rs`).
    pub fn push_edge(&mut self, edge: Edge) -> usize {
        let idx = u32::try_from(self.edges.len()).expect("edge space exhausted");

        if let Some(src) = self.sites[edge.from.index()].enclosing {
            self.ensure_adjacency(src);
            self.forward[src.index()].push(idx);
        }
        for &dst in edge.target.symbols() {
            self.ensure_adjacency(dst);
            self.reverse[dst.index()].push(idx);
        }

        self.edges.push(edge);
        idx as usize
    }

    fn ensure_adjacency(&mut self, s: SymbolId) {
        let need = s.index() + 1;
        if self.forward.len() < need {
            self.forward.resize_with(need, SmallVec::new);
        }
        if self.reverse.len() < need {
            self.reverse.resize_with(need, SmallVec::new);
        }
    }

    /// Edges representing calls made from `s`.
    pub fn outgoing(&self, s: SymbolId) -> impl Iterator<Item = &Edge> {
        self.forward
            .get(s.index())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|&i| &self.edges[i as usize])
    }

    /// Edges representing calls that reach `s`. This is the blast radius
    /// primitive.
    pub fn incoming(&self, s: SymbolId) -> impl Iterator<Item = &Edge> {
        self.reverse
            .get(s.index())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|&i| &self.edges[i as usize])
    }

    /// Whether `s` may be reached through an edge we are not sure about.
    ///
    /// S19: such a symbol must not be split across partitions.
    pub fn is_partitionable(&self, s: SymbolId) -> bool {
        self.incoming(s).all(|e| e.target.is_certain())
    }
}
