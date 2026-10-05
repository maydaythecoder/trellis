# TECH_SPIKE

Implementation detail behind the architecture. Every choice carries chosen /
alternatives / why / what changes our mind, either inline or by reference to an
ADR.

---

## 1. The resolver

### Pass 1 — parse and collect (parallel, rayon)

Per `.py` file under the repository root, a pure function of that file's bytes.
Implemented in `trellis-index/src/parse.rs`.

1. Parse with tree-sitter; keep the `Tree` for incremental reparse.
2. Walk for **definitions**: `function_definition`, `class_definition`, methods.
   Record name, span, enclosing scope chain, and decorators.
3. Walk for **imports**: `import_statement`, `import_from_statement`. Record
   module path, imported names, aliases, relative-import level, and whether it
   is a star import.
4. Walk for **call sites**: `call` nodes. Record callee shape, span, and
   enclosing scope chain. Recurse into arguments — `f(g(x))` is two call sites.

Output: `FileFacts { defs, imports, call_sites }`. No cross-file reasoning.

**Callee shape** is recorded because it bounds what resolution can achieve
before any name lookup happens:

| Shape | Example | Resolvability |
|---|---|---|
| `Name` | `foo()` | Scope walk |
| `Attribute { base, attr }` | `a.b()` | Only when the receiver's origin is known |
| `Dynamic` | `d["k"]()`, `f()()` | Not statically |

Measured shape distribution on the benchmark candidates (`trellis index`,
2026-09-18): syntactically `Dynamic` call sites are 1.4% of flask's 3,963,
0.2% of poetry's 10,421, and 0.05% of scrapy's 24,461. The dynamic-callee
problem is therefore *not* mostly about exotic syntax — it is about
`Attribute` sites whose receiver type is unknown, which is where
`MethodDispatchUntyped` will come from.

### Pass 2 — the module map

Resolve every import's module path against the filesystem. Resolving inside the
repository root makes it **first-party**; everything else — stdlib,
site-packages — is discarded here and never enters the graph.

This is the step that makes the approach tractable: we are not analyzing
Python, we are analyzing this repository's own names.

Relative imports need care and the benchmark is chosen to force it: flask is
176 relative imports against 47 absolute, while poetry is entirely absolute. A
resolver validated only against poetry would ship with the relative path
untested.

### Resolve — bind call sites to definitions

For each call site, walk the scope chain outward: local bindings, parameters,
comprehension scopes, enclosing function scopes, module globals, then the
import table. Emit `Resolved`, `Candidates`, or `Unresolved(Reason)` per ADR
0002.

### Edge schema

```rust
pub enum Target {
    Resolved(SymbolId),
    Candidates(SmallVec<[SymbolId; 4]>),
    Unresolved(Reason),
}

pub struct Edge { pub from: CallSiteId, pub target: Target }
```

`Reason` is a closed enum: `MethodDispatchUntyped`, `DecoratorIndirection`,
`StarImport`, `DynamicAttribute`, `ReexportUnfollowed`, `ShadowedAmbiguous`,
`LocalBinding`, `UnboundName` (`crates/trellis-index/src/graph.rs`).
Closed so that additions are deliberate.

### Incremental update

On file save: `Tree::edit` plus reparse (tree-sitter reparses only changed
bytes), recompute that file's `FileFacts`, invalidate every edge whose source
or target lives in that file, re-resolve those. Never rebuild the world.

### Storage

Dense `u32` newtype ids indexing parallel `Vec`s, with `forward` and `reverse`
adjacency built in the same operation — `Index::push_edge` is the only
insertion path, so bidirectionality is a property of the structure rather than
something a builder must remember. Asserted by a property test over 199 random
graphs (`crates/trellis-index/tests/bidirectional.rs`).

Persisted as `.trellis/index.bin` (postcard), keyed by a content hash of the
file set. **Derived state**: gitignored (A-24; `cache::save` writes `.trellis/.gitignore`), never merged.
On merge, discard both sides, regenerate, diff against the parent branch, hand
the delta to the loop as context.

**Interner.** Hand-rolled (`trellis-core/src/interner.rs`) rather than `lasso`.
The brief nominated `lasso` "or equivalent, verify current maintenance";
verified 2026-09-18, its upstream last push was 2024-08-19 — roughly two years
stale. The surface needed is forty lines. *Changes our mind:* a maintained
interner with a measurably better layout at scale.

---

## 2. Tool contracts

One flat tool list. Every argument struct derives `serde::Deserialize` and
`schemars::JsonSchema`; the derived schema is compiled to a grammar so the
model is constrained to well-formed calls by construction.

| Tool | Args | Returns |
|---|---|---|
| `blast_radius` | `{ symbol: String }` | `Radius(RadiusReport)` — reach *and* coverage |
| `read_file` | `{ path: String }` | `FileContent(String)` |
| `edit` | `{ path, old, new }` | `Edit(EditOutcome)` |
| `run_checks` | `{ paths: Vec<String>, kinds: Vec<CheckKind> }` | `Checks(Verdict)` |

These four are the core. The full set, including `read_skill` (ADR 0019) and
`grep`, `bash` and the rest (ADR 0023), is in `crates/trellis-loop/src/registry.rs`.

`ToolResult` has **no pass variant**. `Checks(Verdict)` is the only
claim-bearing variant, and `Verdict` is constructible only inside
`trellis-checks`, only from a process that ran (ADR 0004).

The grammar guarantees a well-formed path string. It does not guarantee the
path exists — that is S7 metric 3, and it is a judgment property, not a format
one.

---

## 3. Adapter tiers

Full detail in ADR 0006. Compatibility matrix:

| Runtime | Native tools | Guided decoding | Logit access | Tier | Multi-agent |
|---|---|---|---|---|---|
| Anthropic / OpenAI API | yes | — | no | 1 `Native` | n/a (remote) |
| vLLM | varies | yes (llguidance) | via server only | 2 `RuntimeConstrained` | **yes** — continuous batching |
| SGLang | varies | yes (llguidance) | via server only | 2 | **yes** — continuous batching |
| llama.cpp server | partial | yes (llguidance) | no | 2 | no — single-stream |
| Ollama | partial | partial | no | 2 or 4 | no — single-stream |
| In-process (`llama-cpp-2` / `mistral.rs` / `candle`) | n/a | n/a | **yes** | 3 `HarnessConstrained` | depends on build |
| Opaque API, no tools, no guided decoding | no | no | no | 4 `ValidateAndRetry` | n/a |

The multi-agent column is deliberately a **separate matrix from single-agent
support** (S18). A runtime can be perfectly good for one agent and useless for
four.

`trellis doctor` reports the active tier and what degrades on it, because tier
3 and tier 4 are otherwise indistinguishable from the user's seat.

### llguidance embedding contract

Verified against llguidance 1.8.0 (MIT):

```rust
loop {
    let step = constraint.compute_mask()?;        // Result<&StepResult>; >1ms, background thread
    let tok  = sample_with_mask(&logits, step);
    let r    = constraint.commit_token(Some(tok))?; // Result<CommitResult>; fast
    if r.stop { break }
    // r.backtrack and r.ff_tokens are NOT optional to handle:
    // fast-forward tokens are a throughput win, backtracking is correctness.
}
```

Schema compilation uses `JsonCompileOptions::json_to_llg(GrammarBuilder, Value)`.
`GrammarBuilder::new(None, ParserLimits::default())` compiles without a
tokenizer, which is what makes schema validation possible at startup with no
model loaded — implemented and tested in `trellis-model/src/grammar.rs`.

---

## 4. Orchestrator internals

Single-owner actor. It owns the `Index` outright; everything else sends messages
over a `tokio::sync::mpsc` channel and awaits a `oneshot` reply.

```rust
enum OrchestratorMsg {
    BlastRadius { symbol: SymbolId, reply: oneshot::Sender<RadiusReport> },
    ApplyEdit   { edit: FileEdit,   reply: oneshot::Sender<Result<()>> },
    RunChecks   { paths: Vec<PathBuf>, reply: oneshot::Sender<CheckReport> },
    Rollback    { reply: oneshot::Sender<Result<()>> },
}
```

Write and check serialization comes free: a single owner cannot process two
messages at once, and there is no lock anyone can forget to take. The cost is
that a wide change serializes — a real ceiling to measure (S5), not to assume
away.

### Multi-agent

One `tokio::task` per agent, supervised with a `JoinSet` so a panicking agent is
contained and reported rather than aborting the run. A **semaphore bounds
concurrent check subprocesses independently of agent count**: checks are
CPU-heavy, agents are IO-heavy, and they must not compete for one budget.

Each partition gets a git worktree branched from the run's base commit, with its
own index snapshot, under `.trellis/work/run-<ts>/partition-<n>/`. An agent sees
only its worktree. Per-partition rollback is deleting a directory.

Integration is serial and pessimistic: patches apply to the master worktree in
deterministic order, then the **full** check suite runs across the union.

---

## 5. Checks

Three subprocesses via `tokio::process`, each producing a `Verdict`:

- `typecheck(paths)` — the repo's configured type checker, scoped to affected
  paths.
- `run_tests(paths)` — the repo's test runner, scoped to tests covering the
  affected paths.
- `import_check(module)` — import the module in a subprocess. Catches breakage
  that type checking misses and tests do not reach: a syntax error in an
  untested branch, an import cycle, a decorator that raises at module scope.

Commands are **configured, not detected**. A wrong guess produces a confident
verdict about the wrong thing.

Parse structured output where the tool offers it; never regex a human-readable
summary when a JSON mode exists. `CheckDetail::failure_count` exists to carry
that parsed count and is the input to the check-recovery-rate metric (S7 #1).
Currently `None` — parsing per tool is v0.2.

**Deriving the covering-test map** is its own small problem. A coverage report
is the pragmatic source: run the suite once under coverage, invert the
line-to-test mapping, intersect with the changed spans. Open question on
staleness → `OPEN_QUESTIONS.md` Q-4.
