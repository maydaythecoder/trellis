> **Superseded as the entry point.** `00-index.md` is now the top of the
> document set and the per-milestone documents `01`–`05` carry what each
> milestone produced. This file is kept because it is the original
> specification and several documents cite it by part number; it is not
> updated as the build progresses, so where it disagrees with `01`–`05`,
> they are right.

# TRELLIS — Build Brief

> Transcribed into the repository on 2026-09-18 as the project specification.
> Every other document in `docs/` refers back to this one. If you hold the
> original, diff it against this copy before relying on a fine detail.

A Rust agent harness for Python repositories. This document is the whole
specification: what it is, why it is shaped this way, how to build it, and what
counts as done.

Read Part 2 before Part 4. Two of the design decisions look arbitrary and are
not — they come from research findings that will otherwise get quietly designed
away.

---

## Part 1 — What you are building

### Mission

> **TRELLIS returns agent work verified rather than merely written: blast radius
> mapped, tests run, every decision recorded in the repo.**

### The problem

You hand a coding agent a change. It writes something plausible. You now have to
work out what it touched, whether the callers still work, and why it made the
choices it made — and its reasoning evaporated when the session ended. Tomorrow
you reopen the repo and start by re-explaining your own project to a machine
that was here yesterday.

### The shape

A CLI you run inside a Python repo. It builds a call graph of your first-party
code, drives an agent loop against that graph, verifies every change with your
type checker and your test suite, and commits a decision trail so the next
session starts informed.

It runs on models you host, including models with no native tool-calling
support. Your code stays on your machine. Your API key, if you use one, stays on
your machine.

### Goal state

1. **Resumption is free.** Reopen a repo, say what's next, re-explain nothing.
2. **Scope is known before the change.** The harness names the first-party call
   sites reaching a function, and names honestly the ones it could not resolve.
3. **Verification executes.** "The callers still work" means the type checker
   ran clean and covering tests passed.
4. **Failure is boring.** Non-convergence rolls back completely and hands back a
   legible reason.
5. **The trail is durable.** Committed, agent-readable, in the repo.
6. **It runs on your hardware**, on quantized open weights, without native tool
   calling.
7. **Cost and wall-clock are bounded and visible.**

### Non-goals for v1

Any language but Python. Repos without a test suite. Cross-repo analysis. Any
surface beyond the CLI. Any networked component. Multi-user anything. Running
while the machine is asleep.

*2026-10-07 (#557), "Cross-repo analysis" read by the owner's decision of
2026-10-07:* the non-goal does not cover listing recent repositories with
each one's last outcome, which `trellis overview` and the desktop show as a
launcher list (ADR 0021 rows 44–47). Accepted; built 2026-10-07 (#558).
*As built 2026-10-07 (#558):* `trellis overview` prints that list, each
repository with its newest trail's outcome, and compares nothing across
repositories.

*2026-10-07 (#557), "Any networked component" read by the owner's decision
of 2026-10-07:* the board may start the user's own `gh` CLI, read-only, for
PR and CI state; trellis makes no network request of its own (ADR 0021 row
12.3 and PC-126). Accepted; built 2026-10-07 (#558).
*As built 2026-10-07 (#558):* `trellis overview` starts the user's first
`gh` on PATH through `secrets::scrub` with no token, read-only, each call
bounded to 10 seconds; without it, or not logged in, the board says so.

---

## Part 2 — Two constraints that are not negotiable

These exist because the obvious design fails, and the failure is documented
rather than hypothetical. If you find yourself designing around either one, stop
and re-read this part.

### Constraint 1 — Complete call-graph recall is impossible in Python

PyCG, the academic state of the art for static Python call graphs, reports
roughly 99.2% precision against about 69.9% recall. Roughly three call edges in
ten are statically invisible. This is a property of the language — dynamic
dispatch, decorator indirection, `getattr`, conditional imports — not a gap in
tooling that better engineering closes.

The prevalence research (Yang et al., MSR '22, over 3 million Python files) is
more encouraging than it sounds: genuinely unanalyzable constructs like `eval`
are rare, while decorators and context managers are widespread. So your misses
will be boring and common, not exotic.

**What this means for the design.** The graph must be *high precision, explicitly
incomplete*. Every edge carries a confidence label. Every run reports coverage.
The harness never emits a bare "complete." A tool that confidently reports a
blast radius missing the thing that breaks is worse than no tool at all.

### Constraint 2 — An LLM cannot verify work without an external signal

Huang et al. (ICLR 2024, DeepMind) found that without external feedback, LLMs
struggle to self-correct and often *degrade*: GPT-4 on GSM8K fell from 95.5% to
91.5% to 89.0% over two rounds of intrinsic self-correction. A companion result
found models are specifically bad at *detecting* errors, though they can fix one
they're pointed at. The MAST study (Cemri et al., 2025) annotated 1,642
multi-agent traces and found 41–86.7% failure rates across seven frameworks,
concluding better base models won't fix it. A scaling study measured independent
multi-agent topologies amplifying errors 17.2-fold versus a single agent.

The counterpart result is the important one: Chen et al. (ICLR 2024) showed
self-debugging works consistently and substantially **when anchored to unit-test
execution**. Ablations show the code execution is what does the work.

**What this means for the design.** Correctness verdicts come only from things
that execute — type checker, test runner, import check. The model reads their
output and decides what to do next. It never issues a pass verdict. This is
enforced in the type system (Part 4.6), not in a prompt or a code-review
guideline, because under delivery pressure it is the first thing that gets
softened.

This constraint is also what makes small open-weight models viable. A design
that rested correctness on model judgment would be hardest to run on weak
models; a design that rests it on a type checker barely cares which model you
use.

---

## Part 3 — Architecture

```
┌─────────────────────────────────────────────┐
│ CLI                                          │
└────────────────┬─────────────────────────────┘
                 │
┌────────────────▼─────────────────────────────┐
│ Orchestrator  (single owner, actor)          │
│  owns index · serializes writes · budgets    │
│  · rollback · trail                          │
└──┬───────────┬──────────┬──────────┬─────────┘
   │           │          │          │
┌──▼─────┐ ┌───▼────┐ ┌───▼────┐ ┌───▼────────┐
│ Index  │ │ Loop   │ │ Checks │ │ Trail      │
│ graph  │ │ +tools │ │ verdict│ │ committed  │
└────────┘ └───┬────┘ └────────┘ └────────────┘
               │
          ┌────▼─────┐
          │ Model    │
          │ adapter  │
          │ +llguid. │
          └──────────┘
```

**The loop is a single agent loop, not a committee.** One model, a flat tool
list, iterating. The blast radius and the checks are *tools it calls*, not
stages in a pipeline you wrote for it. This is the consistent shape across
open-source harnesses worth copying — the teardown in
`shareAI-lab/learn-claude-code` puts it directly: a good harness does not try to
be the agent, does not impose rigid workflows, and does not substitute
hand-crafted decision trees for the model's own judgment. It supplies tools,
context management, and permission boundaries, then gets out of the way.

**Many agents may run at once; none of them judge each other.** TRELLIS
parallelizes *work* — several agents on disjoint regions of a change, each in its
own isolated worktree (4.13). It never parallelizes *correctness*: no agent
reviews another agent's output for a pass verdict, no voting, no consensus.
Constraint 2 is why, and the distinction is the whole design. Parallel work
multiplies throughput. Parallel judgment multiplies error — a scaling study
measured independent multi-agent topologies amplifying errors 17.2-fold versus a
single agent, with centralized coordination containing it to 4.4-fold. Keep the
coordination centralized and the judgment mechanical.

Sub-agents also exist for context isolation on separable subtasks. That is
delegation, not review.

**Reference implementations to read before writing the loop:**

- `shareAI-lab/learn-claude-code` — the design philosophy teardown. Read first.
- `HarnessLab/claw-code-agent` — Python reimplementation of the Claude Code
  architecture targeting local models over an OpenAI-compatible server; zero
  external dependencies; covers prompt assembly, context building, tool calling,
  session persistence, nested delegation.
- `ultraworkers/claw-code` — the same CLI harness shape in Rust. Closest to your
  language.
- `majiayu000/agent-harness` — useful for the validation-gate and handoff
  pattern.

Study the loop, context assembly, and permission model. Ignore their workflow
opinions.

---

## Part 4 — Rust implementation

### 4.1 Workspace layout

```
trellis/
├── Cargo.toml                 # workspace
└── crates/
    ├── trellis-core/          # shared types, IDs, errors
    ├── trellis-index/         # tree-sitter → call graph
    ├── trellis-checks/        # typecheck, tests, imports — owns Verdict
    ├── trellis-model/         # adapter tiers, llguidance, inference
    ├── trellis-loop/          # agent loop, tools, budgets
    ├── trellis-trail/         # decision trail
    ├── trellis-vcs/           # git snapshot and rollback
    └── trellis-cli/           # binary
```

Crate boundaries here are load-bearing, not cosmetic. `trellis-checks` owning
the `Verdict` type is what makes Constraint 2 structural (4.6). Keep
`trellis-index` free of any dependency on `trellis-model` — the graph must be
testable without a model anywhere near it.

### 4.2 Dependencies

| Purpose | Crate | Notes |
|---|---|---|
| Parsing | `tree-sitter`, `tree-sitter-python` | Pin both; grammar and library versions must match or the bindings fail at runtime |
| Grammar constraint | `llguidance` | Rust-native. See 4.7 |
| Tool schemas | `serde`, `schemars` | Derive JSON Schema from tool arg structs, hand to llguidance |
| Async runtime | `tokio` | Orchestrator actor, process spawning, model IO |
| Data parallelism | `rayon` | Parse pass over files |
| Graph storage | hand-rolled adjacency (see 4.4) | `petgraph` is more than you need and fights your ID scheme |
| Interning | `lasso` or equivalent | Symbol and path interning; verify current maintenance |
| Index serialization | `postcard` or `bincode` | Binary cache; measure both |
| Trail serialization | `serde_json` | Human- and agent-readable by requirement |
| Git | `git2` | Snapshot and rollback. Shelling out to `git` is an acceptable v0.2 shortcut |
| CLI | `clap` (derive) | |
| Config | `toml` + `serde` | `.trellis/config.toml` in the repo |
| Errors | `thiserror` in libs, `anyhow` in the binary | |
| Instrumentation | `tracing` + `tracing-subscriber` | Doubles as the budget accounting substrate — do not build a second telemetry path |
| Snapshot testing | `insta` | Graph output regression tests |

Verify every version and license before committing. Several of these move
quickly.

### 4.3 The index: algorithm

Two passes plus a resolve step. All of it in `trellis-index`.

**Pass 1 — parse and collect (parallel, rayon).** For each `.py` file under the
repo root:

1. Parse with tree-sitter. Keep the `Tree` for incremental reparse.
2. Walk for **definitions**: `function_definition`, `class_definition`, methods.
   Record name, span, enclosing scope chain.
3. Walk for **imports**: `import_statement`, `import_from_statement`. Record
   module path, imported names, aliases, relative-import level.
4. Walk for **call sites**: `call` nodes. Record the callee expression shape
   (plain identifier, attribute access, subscript, something else), span, and the
   enclosing scope chain.

Output per file: `FileFacts { defs, imports, call_sites }`. No cross-file
reasoning yet.

**Pass 2 — build the module map.** Resolve every import's module path against
the filesystem. If it resolves to a file inside the repo root, it is
**first-party**. Everything else — stdlib, site-packages — is discarded here and
never enters the graph. This step is the whole reason the approach is tractable:
you are not analyzing Python, you are analyzing *this repo's own names*.

**Resolve — bind call sites to definitions.** For each call site, walk the scope
chain outward: local bindings, parameters, comprehension scopes, enclosing
function scopes, module globals, then the import table. Emit one of three
outcomes:

- **`Resolved(SymbolId)`** — exactly one first-party definition matches, nothing
  nearer shadows it.
- **`Candidates(SmallVec<SymbolId>)`** — a known finite set. Conditional import,
  two same-named definitions in scope, a partially-followed re-export chain. All
  recorded, none chosen.
- **`Unresolved(Reason)`** — with a reason code.

```rust
pub enum Reason {
    MethodDispatchUntyped,   // obj.foo() where obj's type is unknown
    DecoratorIndirection,    // callee is wrapped; expect this to dominate
    StarImport,              // from x import *
    DynamicAttribute,        // getattr, __getattr__, dict dispatch
    ReexportUnfollowed,      // __init__.py chain too deep or ambiguous
    ShadowedAmbiguous,       // binding shadowed in a way we can't disambiguate
}
```

Instrument the reason distribution from day one. It is the input to the v0.1
gate and to the decision in **S1**.

**Incremental update.** On file save: `Tree::edit` + reparse (tree-sitter
re-parses only changed bytes), recompute that file's `FileFacts`, invalidate
every edge whose source or target lives in that file, re-resolve those. Do not
rebuild the world.

### 4.4 The index: data structures

```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash)] pub struct FileId(u32);
#[derive(Copy, Clone, PartialEq, Eq, Hash)] pub struct SymbolId(u32);
#[derive(Copy, Clone, PartialEq, Eq, Hash)] pub struct CallSiteId(u32);

pub enum Target {
    Resolved(SymbolId),
    Candidates(SmallVec<[SymbolId; 4]>),
    Unresolved(Reason),
}

pub struct Edge { pub from: CallSiteId, pub target: Target }

pub struct Index {
    files:     Vec<FileEntry>,
    symbols:   Vec<SymbolEntry>,
    sites:     Vec<CallSiteEntry>,
    forward:   Vec<SmallVec<[u32; 4]>>,  // symbol → outgoing edge indices
    reverse:   Vec<SmallVec<[u32; 4]>>,  // symbol → incoming edge indices
    edges:     Vec<Edge>,
    interner:  Interner,
}
```

Bidirectionality is free: build `forward` and `reverse` in the same loop. The
v1.0 gate's "every emitted edge verified real and bidirectional" is then a
property test over the pair, not an architectural achievement.

Persist the index as a binary blob under `.trellis/index.bin`, keyed by a
content hash of the file set. It is **derived state**: stored in the repo for
other agents to read, never merged. On merge, discard both sides, regenerate
from current state, diff against the parent branch, and hand the delta to the
loop as context.

### 4.5 The orchestrator: concurrency

Single-owner actor. The orchestrator owns the `Index` outright — no
`Arc<RwLock<Index>>`, no shared mutable graph. Everything else sends it messages
over a `tokio::sync::mpsc` channel and awaits a `oneshot` reply.

```rust
enum OrchestratorMsg {
    BlastRadius { symbol: SymbolId, reply: oneshot::Sender<RadiusReport> },
    ApplyEdit   { edit: FileEdit,   reply: oneshot::Sender<Result<()>> },
    RunChecks   { paths: Vec<PathBuf>, reply: oneshot::Sender<CheckReport> },
    Rollback    { reply: oneshot::Sender<Result<()>> },
}
```

This gives you write/check serialization for free — the single owner cannot
process two messages at once — without a lock you can forget to take. The cost is
that a wide change serializes, which is a real ceiling you must measure
(**S5**), not assume away.

### 4.6 The grounding contract, enforced by the type system

This is the most important twenty lines in the codebase.

```rust
// trellis-checks/src/verdict.rs

pub struct Verdict {
    passed: bool,
    detail: CheckDetail,
    _seal: Seal,          // private unit struct — outside crates cannot construct
}

struct Seal;

impl Verdict {
    // Only these three constructors exist, all private to this crate,
    // all requiring a real process to have run.
    pub(crate) fn from_typecheck(o: ProcessOutput) -> Self { /* ... */ }
    pub(crate) fn from_tests(o: ProcessOutput)     -> Self { /* ... */ }
    pub(crate) fn from_import(o: ProcessOutput)    -> Self { /* ... */ }

    pub fn passed(&self) -> bool { self.passed }
}
```

The model's tool-result type has **no pass variant**:

```rust
// trellis-loop/src/tools.rs
pub enum ToolResult {
    Radius(RadiusReport),
    Checks(Verdict),        // came from trellis-checks, cannot be forged
    FileContent(String),
    Edit(EditOutcome),
    Error(String),
}
```

There is no code path by which the model's output becomes a `Verdict`. A future
contributor who wants to let the model say "looks fine" has to delete a private
field and change a crate boundary, which is loud enough to notice in review.
That is the point.

Two audit modes, both surfaced to the user (**S3**):

- **`Verified`** — a check covered this call site and passed.
- **`Reviewed`** — no test covers it; the model read it and did not object. A weak
  signal, labeled as one, counted separately in the trail and on the approval
  screen.

Never collapse these two into one number.

### 4.7 The model adapter and llguidance

**llguidance** (`guidance-ai/llguidance`) is the single grammar authority. It is
a Rust crate, so it embeds natively with no FFI to a Python stack. It computes a
token mask from a context-free grammar, a tokenizer, and a token prefix at
roughly 50μs of single-core CPU time per token on a 128k tokenizer, with
negligible startup cost. It accepts JSON Schema, regular expressions, and a
Lark-like CFG syntax. It reached v1.0.0 in mid-2025.

Embedding contract, three calls:

```rust
loop {
    let mask = constraint.compute_mask()?;   // >1ms; run on a background thread
    let tok  = sample_with_mask(&logits, &mask);
    constraint.commit_token(tok)?;           // very fast
    if constraint.is_stopped() { break }
}
```

The reason this collapses the runtime problem: llguidance is already the grammar
engine inside llama.cpp, vLLM, SGLang, and Guidance, and powers OpenAI's
JSON-Schema structured outputs. The grammar you compile from a tool schema is
the *same artifact* whether TRELLIS masks locally or ships it to the serving
runtime. One engine, two delivery paths, no divergence between them.

It is also the right engine on the merits: llama.cpp's own grammar sampler is
slower because it lacks a lexer and uses a backtracking parser; Outlines
pre-computes masks for every automaton state, which caps constraint complexity
and adds startup cost and memory. llguidance uses an Earley parser over a
derivative-based lexer, builds automata lazily, and computes masks on the fly.

**Tool schema → grammar**, no hand-written BNF:

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct BlastRadiusArgs { symbol: String }
// schemars → JSON Schema → llguidance (which accepts a large subset directly)
```

**Four adapter tiers**, selected by probing model and runtime at startup:

1. **Native tool calling** — the model's own function-calling API. Use when
   present.
2. **Runtime-side constraint** — pass the compiled grammar to the runtime's
   guided-decoding parameter (llama.cpp, vLLM, SGLang). Their embedded
   llguidance does the masking.
3. **Harness-side constraint** — TRELLIS owns the sampling loop via in-process
   inference and applies the mask itself. This is the tier that makes any model
   work regardless of what it was trained for, and it is the default for local
   open weights.
4. **Validate-and-retry** — opaque API with neither native tools nor guided
   decoding: use llguidance as a validator on returned text, retry on failure.
   Lossy and slow; warn the user loudly.

Tier 3 needs logit access, which means **in-process inference, not an HTTP
endpoint**. Choosing that path — `llama-cpp-2`-style bindings, `mistral.rs`,
`candle` — is an open decision with real consequences for build complexity and
hardware coverage. Evaluate in v0.2 and record an ADR (**S17**).

**Why this matters more than it looks.** Constraint happens at the sampler, not
the model: the grammar sampler holds a parser state, computes which tokens could
legally continue, and sets every other logit to negative infinity before
sampling runs. Output is valid by construction rather than by luck, which is why
it works on a 1B model never fine-tuned for structured output. And the failure
it removes is the one that actually bites locally — **quantization degrades
format reliability faster than it degrades reasoning**. Your 14B quant may reason
adequately about the code and still emit malformed tool calls. Grammar
constraint deletes that failure mode instead of papering over it with retries.

### 4.8 Checks

Three subprocesses, spawned via `tokio::process`, each producing a `Verdict`:

- **`typecheck(paths)`** — the repo's configured type checker, scoped to the
  affected paths.
- **`run_tests(paths)`** — the repo's test runner, scoped to tests covering the
  affected paths. Deriving that coverage map is its own small problem; a
  coverage report is the pragmatic source.
- **`import_check(module)`** — import the module in a subprocess. Catches the
  class of breakage that type checking misses and tests don't reach.

Parse structured output where the tool offers it. Never regex a human-readable
summary when a JSON mode exists.

### 4.9 Rollback

Git-backed and boring. Before a run: require a clean worktree, record `HEAD`.
During the run: the orchestrator is the only writer. On any failure or budget
exhaustion: `git reset --hard` to the recorded commit and clean untracked files
the run created.

Requiring a clean worktree is a real constraint on the user and the right trade.
The alternative — snapshotting a dirty tree and restoring it — is a source of
data-loss bugs you do not want in the one path that has to be reliable.

Verify rollback with an induced mid-run panic in the test suite. It is a v1.0
gate condition.

### 4.10 The trail

One JSON document per run, committed under
`.trellis/runs/<timestamp>-<slug>.json`. Two structurally separate sections:

```jsonc
{
  "task": "...",
  "factual": {              // what happened. Machine-checkable.
    "proposed": [...],
    "radius": { "resolved": 47, "candidates": 3, "unresolved": 2 },
    "checks": [ { "kind": "tests", "passed": true, "detail": "..." } ],
    "approved_by": "human",
    "mode_counts": { "verified": 45, "reviewed": 7 }
  },
  "judgment": {             // the model's opinion. Speculation, labeled.
    "tech_debt_projection": "...",
    "rationale": "..."
  }
}
```

The split matters because future agents read the trail as context. An errant
judgment entry otherwise propagates indefinitely. Treat trail contents as
**untrusted input on read-back** — it lives in a repo that may take contributions
(**S10**).

### 4.11 Budgets

Hard per-run ceilings on iterations, wall-clock, and tokens, read from
`.trellis/config.toml`. Track through `tracing` spans so accounting and
observability are the same mechanism. On exhaustion: escalate once with the
current state, then roll back.

The cost profile inverts by configuration and the budget code must know which it
is in: on a frontier API, tokens are the scarce resource; on local weights,
marginal token cost approaches zero and **wall-clock** is the budget. Since keys
are the user's, a runaway loop spends their money — the global ceiling applies
regardless of tier.

### 4.12 CLI surface

```
trellis index                 # build or refresh the index; print coverage
trellis radius <symbol>       # blast radius, with confidence breakdown
trellis run "<task>"          # the loop
trellis resume                # continue from the last trail entry
trellis check                 # run the grounded checks standalone
trellis doctor                # probe model, runtime, tier, type checker, tests
```

`trellis doctor` is not a nicety. It reports which adapter tier is active and
what degrades on it — the difference between tier 3 and tier 4 is invisible to a
user otherwise.

### 4.13 Multi-agent work

Several agents working concurrently on one task. This is where the index earns
its keep beyond blast radius: it is what lets you decide *safely* which work can
run in parallel.

**The rule.** Agents parallelize work, never judgment. No agent reviews another's
output for correctness. Verdicts still come only from `trellis-checks`.

**Partition by blast radius.** Given a task that decomposes into N units of work,
compute each unit's radius from the index and partition into groups whose radii
do not intersect. Non-intersecting groups run in parallel. Anything that
intersects is serialized into the same partition and handled by one agent.

```rust
pub struct Partition {
    pub id: PartitionId,
    pub units: Vec<WorkUnit>,
    pub radius: HashSet<SymbolId>,   // union of unit radii
    pub files: HashSet<FileId>,      // files this partition may touch
}
```

Two partitions are compatible when their `radius` sets and their `files` sets
are both disjoint. Files matter separately from symbols: two symbols in one file
cannot be edited by two agents without a merge conflict, even with disjoint call
graphs.

**Isolate with git worktrees.** Each partition gets its own worktree, branched
from the run's base commit, with its own index snapshot. An agent sees only its
worktree. This gives you real filesystem isolation for free and makes rollback
per-partition trivial: delete the worktree.

```
.trellis/work/
  run-20260918-143022/
    partition-0/     # git worktree, branch trellis/run-.../p0
    partition-1/
    partition-2/
```

**The orchestrator stays single-owner.** It owns the *master* index and the
integration. Each agent owns its worktree's index snapshot. Nothing shared and
mutable crosses a thread boundary — agents talk to the orchestrator over
channels, exactly as in 4.5.

```rust
enum AgentMsg {
    Ready    { partition: PartitionId, patch: Patch, local: Verdict },
    Failed   { partition: PartitionId, reason: FailureReason },
    NeedsSerialization { partition: PartitionId, conflicting: SymbolId },
}
```

**Optimistic partitioning, pessimistic integration.** This is the part that has
to be right. Per-partition checks run in parallel and are fast — they are a
filter, not a guarantee. When partitions return, the orchestrator applies patches
to the master worktree in a deterministic order and runs the **full** check suite
across the union. Partition-local passes never substitute for the integrated
check, because the partitioning is only as trustworthy as the graph's recall,
and the graph's recall is partial by Constraint 1 (**S19**).

**Failure semantics.** Because partitions are radius-disjoint, partial success is
meaningful: partition 0 and 2 converge, partition 1 doesn't. Options, chosen by
config and surfaced to the user:

- **`all-or-nothing`** (default) — any partition failing rolls back the whole run.
  Safest, correct under partial recall.
- **`land-clean`** — successful partitions integrate and land; failed ones are
  reported with their worktrees preserved for inspection. Only valid when the
  integrated check passes on the landed subset.

Never let a failed partition's work land partially. Delete the worktree.

**Concurrency in Rust.** One `tokio::task` per agent, supervised with a `JoinSet`
so a panicking agent is contained and reported rather than aborting the run.
Each agent's subprocess checks run under `tokio::process`. A semaphore bounds the
number of concurrently running check subprocesses independently of the number of
agents, because checks are CPU-heavy and agents are IO-heavy and they should not
compete for the same budget.

```rust
let mut set = JoinSet::new();
for p in partitions {
    let tx = orch_tx.clone();
    let sem = check_sem.clone();
    set.spawn(async move { run_agent(p, tx, sem).await });
}
```

**Budgets divide, they don't multiply.** The global per-run ceiling is split
across partitions, not granted to each. N agents on a runaway loop is an N-times
faster way to spend the user's money. Track per-partition spend in `tracing`
spans nested under the run span.

**Inference is the real bottleneck, not the harness.** N parallel agents against
one local model server do not get N× throughput — they queue on one GPU. This has
a concrete consequence for the serving stack: llama.cpp and Ollama optimize for
batch-size-1 single-user work and choke under concurrency, while vLLM and SGLang
do continuous batching and are built for it. So **multi-agent mode effectively
requires a batching runtime**. Probe for it and say so in `trellis doctor`;
degrade to sequential agents on a non-batching backend rather than pretending
(**S18**).

**N=1 is the degenerate case.** Build the partitioning and worktree machinery
from v0.2 with a single partition, so the architecture is right from the start,
and enable N>1 in v1.1 once the single-agent numbers are known.

**CLI.**

```
trellis run "<task>" --agents 4       # cap concurrent agents
trellis run "<task>" --agents auto    # derive from backend batching capacity
trellis partitions "<task>"           # dry run: show the partition plan and why
```

`trellis partitions` is a debugging tool you will use constantly. It shows which
units were grouped, which radii collided, and which symbols forced
serialization.

---

## Part 5 — Milestones

### v0.1 — Measure (no product ships)

Replace assumptions with numbers.

- Name a benchmark Python repo: mid-size, first-party-import-heavy, real test
  coverage. Propose three with selection criteria.
- Build the index against it. Measure: precision on a hand-audited sample,
  recall against hand-audited ground truth, build time, incremental update
  latency, peak memory, and the `Reason` distribution.
- Probe the model floor (Part 7, **S7**).

**Exit:** a measured recall figure, a stated threshold with reasoning, a known
model floor, and an ADR on whether a semantic backstop is needed.

Ground truth must be a **hand-audited sample**, not another tool's output.
Comparing your graph to Pyright's measures agreement, not correctness, and both
share blind spots.

### v0.2 — The loop

Main agent loop, tool set, llguidance adapter across at least two local models
plus one frontier API, orchestrator actor with serialized writes and git
rollback, full `tracing` instrumentation.

**Exit:** one real change on the benchmark repo runs end to end; an induced
mid-run failure rolls back cleanly.

### v0.3 — Memory

Context file, prompt, and governance rule import. Committed trail with the
factual/judgment split. Trail read back on resume.

**Exit:** the wedge, demonstrated once — reopen, resume, re-explain nothing.

### v1.0 — The harness

Hard gate. All five, no substitutions:

1. Recall on the benchmark repo meets the v0.1 threshold.
2. Zero unsound edges — every `Resolved` edge verified real on the benchmark
   corpus.
3. A real change runs to human approval with a complete committed trail and
   grounded checks.
4. Induced mid-run failure returns the repo to its exact pre-run state.
5. Resumption works.

Plus: acceptable operation on a local open-weight model at or above the measured
floor, via constrained tool calling.

### Later

v1.1: multi-agent mode enabled (N>1). Requires the single-agent numbers first —
you cannot tune partition size or agent count without knowing what one agent
costs and how long it takes. Gate: a change that decomposes into three or more
disjoint partitions completes correctly in parallel, with a
deliberately-induced cross-partition collision caught by the integrated check
and recorded as a suspected recall gap (**S19**).

v1.x: semantic backstop if v0.1 demands it, frontier escalation policy,
coverage-gap UX, budget calibration from real usage.

v2.0, each needing its own brief: more languages, cloud execution, shared
environments, messaging and scheduling, desktop and web surfaces, MCP exposure,
template sharing, multi-user.

---

## Part 6 — Documents to write

Alongside the code:

1. **`MISSION.md`** — Part 1, plus positioning that concedes the index is
   commodity (**S15**) and argues the wedge on the harness and the trail.
2. **`TECH_SPIKE.md`** — the resolver algorithm in detail, edge schema, tool
   contracts, adapter tiers, orchestrator internals, per choice: chosen /
   alternatives / why / what changes our mind.
3. **`RISK_REGISTER.md`** — every soft spot in Part 7 as an entry with severity,
   likelihood, a concrete failure scenario, and a costed patch.
4. **`BENCHMARK_AND_EVAL.md`** — v0.1 in full.
5. **`adr/`** — resolver scope and build-not-consume; edge confidence model;
   degradation policy; the grounding contract; loop shape and why not
   multi-agent verification; adapter tiers and the inference path; trail format;
   budgets; the clean-worktree requirement.
6. **`ROADMAP.md`** — Part 5 verbatim, gate unsoftened, the **S16** pivot trigger
   as a named gate.
7. **`COST_MODEL.md`** — cost and wall-clock per change at 10, 50, 400 call
   sites, per configuration.
8. **`CLAUDE.md`** — repo conventions, commands, what needs an ADR to change.
9. **`OPEN_QUESTIONS.md`** — grouped by who answers.

---

## Part 7 — Soft spots

Known weaknesses. Each is a risk-register entry with a costed patch. Do not
paper over any of them.

**S1 — The resolver's reach is unmeasured, and the fallback is unchosen.**
Import-scoped binding is tractable, but nobody knows yet what fraction of real
call sites it resolves on your benchmark repo. If it lands high, you're done. If
low, you need a semantic backstop: Pyright over LSP (mature, MIT, type-aware —
but a Node process, multi-GB memory on large repos, no batch call-graph dump), or
GitHub's stack-graphs (Rust-native, file-incremental, production-proven at GitHub
since 2021 — but recently assessed inactive, Python ruleset pinned to old
grammars, open cross-file resolution bugs). *Patch:* measure in v0.1; record the
fallback with a trigger threshold, so the choice is data-driven rather than a
frustrated decision in month three.

**S2 — Recall is partial and the product must never imply otherwise.** *Patch:*
the confidence model and per-run coverage reporting in 4.3–4.4. Every consumer
reads coverage alongside the radius.

**S3 — "Callers still work" means two different things.** Covered by tests, the
audit verifies; uncovered, a model read it and didn't object. *Patch:* the
`Verified`/`Reviewed` split, counted separately everywhere. Write product copy to
match. The first time something breaks after a clean run, this wording decides
whether the user feels informed or lied to.

**S4 — The grounding contract is easy to erode.** *Patch:* 4.6. Structural, not
advisory.

**S5 — The single-owner orchestrator serializes, and small windows squeeze
context.** A change touching 400 call sites is 400 checks behind one owner, and
local models have small context windows. Partitioning (4.13) attacks this
directly — disjoint radii run in parallel — but only to the extent the work
actually decomposes, and integration remains serial by design. *Patch:* measure
call-sites-to-wall-clock and call-sites-to-context from the first working loop;
measure the decomposition ratio on real tasks (what fraction of changes partition
into more than one group at all); design the context selector against your
*smallest* supported window; batch check results rather than streaming them raw.

**S18 — Multi-agent throughput is bounded by the inference backend, not the
harness.** N agents against one local model queue on one GPU. llama.cpp and
Ollama are built for batch-size-1 single-user work and degrade under
concurrency; vLLM and SGLang do continuous batching and are built for it. A user
running four agents against Ollama will see no speedup and may see worse, and
will blame TRELLIS. *Patch:* probe the backend's batching capability at startup;
report it in `trellis doctor`; make `--agents auto` derive from measured backend
capacity rather than CPU count; degrade to sequential agents on a non-batching
backend and say so explicitly rather than pretending. Document the supported
serving stacks for multi-agent mode as a separate matrix from single-agent mode.

**S19 — Partition safety inherits the graph's recall.** Two partitions are
declared disjoint because their blast radii don't intersect — but the radii are
computed from a graph that misses roughly three edges in ten (Constraint 1). Two
agents can therefore collide through an edge the index never saw, and the
failure is silent and confusing: both partitions pass locally, the integrated
result breaks, and nothing in the trail explains why. This is the most dangerous
interaction in the whole design, because it converts an honest incompleteness
into an apparent nondeterminism. *Patch, three parts:* (1) treat any symbol with
`Unresolved` or `Candidates` incoming edges as **unpartitionable** — it and its
dependents go into one partition, conservatively; (2) never let partition-local
checks substitute for the integrated full check; (3) when an integrated check
fails after clean partition checks, record it in the trail as a **suspected
recall gap** with the symbols involved, and feed those cases back into the S1
measurement. Over time that becomes your best real-world recall signal.

**S6 — Degradation UX is undesigned.** Partial coverage will be routine, and
whether that reads as honest or useless is a UI question. *Patch:* design what
the user sees on the tenth partial-coverage run in an hour, and the one-click
paths — proceed with gaps recorded, inspect candidates, add a test.

**S7 — The model floor is unknown, and constraint moves it without removing it.**
Constrained decoding guarantees a well-formed call, not a sensible one. A 3B
model will now reliably emit valid JSON calling the wrong function. The floor is
about judgment, not format. *Patch:* a v0.1 spike across sizes and quantization
levels, all under constraint, on 20–30 real tasks from the benchmark repo.
Measure seven things, in priority order:

1. **Check-recovery rate — the primary signal.** Given a failing type check or
   test, does the next iteration reduce the failure count? The whole
   architecture rests on the model acting sensibly on check output. A model that
   can't read a type error and make progress is unusable here regardless of
   anything else. Measure failures resolved per iteration, and the rate of
   iterations that *increase* failures.
2. **Task completion rate.** The headline number, but lagging.
3. **Argument correctness.** Valid is not correct — the grammar guarantees a
   well-formed path string, not an existing file. Grade against a gold trace.
4. **Termination discipline, both directions.** Stopping early with the job half
   done, and looping past the point of progress. Different failures, different
   fixes.
5. **Run-to-run variance.** A floor set on a mean is a lie. Five runs per task;
   set the floor on the worst case or p25.
6. **Context degradation.** Same task at 25%, 50%, 75% window fill. Determines
   the context budget in **S5**.
7. **Wall-clock per task**, including constraint overhead.

Report the floor as a **configuration** — model × quantization × runtime — not a
parameter count. A well-quantized 14B on a good runtime beats a badly-quantized
32B, and "7B minimum" tells a user nothing.

**S8 — Budgets are uncalibrated and the cost profile inverts by configuration.**
*Patch:* instrument from day one; set tiers per configuration from measured
data; hard global ceiling regardless.

**S9 — Scale is unvalidated.** Parsing scales fine — tree-sitter runs at roughly
90+ source lines per millisecond and reparses incrementally. Resolution, memory,
and index storage have no published numbers at a million lines for any Python
tool. *Patch:* support 100k–300k lines in v1.0; treat a million as a stretch
target needing sharding; measure before claiming.

**S10 — The trail is trusted context, which makes it an injection and
propagation surface.** *Patch:* the factual/judgment split, provenance per entry,
treat contents as untrusted on read-back, consider hashing factual entries.

**S11 — Tech-debt projection is speculation filed next to facts.** *Patch:*
separate section, explicit label, no consumer treats it as established.

**S12 — The BYOK safe harbor depends on keys never touching your
infrastructure.** Local execution with the user's key going directly to the
provider is standard practice; proxying or storing keys server-side is where
products get into trouble. *Patch:* hard architectural rule in an ADR, carried
into any future cloud tier where breaking it becomes tempting.

**S14 — The benchmark repo is unnamed and the v1.0 gate depends on it.** No repo,
no recall figure; no recall figure, no gate. *Patch:* three candidates with
criteria in week one.

**S15 — The index is not a moat.** Aider's tree-sitter repo map, a wave of MCP
code-graph tools, Sourcegraph, and GitHub's own code navigation all occupy this
space. *Patch:* position on the harness — grounded verification, clean rollback,
bounded budgets, the committed trail — and treat the index as infrastructure.
Building it yourself is justified by fit and simplicity, not by differentiation.

**S16 — The pivot trigger, written while it's cheap.** If measured recall is too
low to support a blast-radius claim, the guarantee dies. What survives: context
continuity, governance enforcement, test-anchored checks, and the committed
trail — a smaller product that still solves the problem in Part 1, with the index
demoted from guarantee to hint. *Patch:* name the recall figure that fires this
pivot **before** v0.1 produces the number.

**S17 — Residue of the constraint problem.** Embedding llguidance removes the
dependency on the user's serving stack, but three things remain. *Logit access:*
tier 3 needs in-process inference, an unmade decision with real build-complexity
consequences — ADR in v0.2. *Dependency risk:* llguidance sits in the hot path;
verify its license, confirm maintenance cadence, know that XGrammar and a
hand-rolled DFA masker are the worse fallbacks. *User-visible tiering:* someone
on an opaque API silently drops to validate-and-retry — publish a compatibility
matrix and report the active tier in `trellis doctor`.

---

## Part 8 — Definition of done

- [ ] Every document in Part 6 exists.
- [ ] An `ASSUMPTIONS` block listing every default chosen where this brief was
      silent.
- [ ] All soft spots in `RISK_REGISTER.md` with a costed patch each.
- [ ] S1 resolved: measured resolver reach, fallback trigger recorded.
- [ ] S14 resolved: named benchmark repo, recall threshold with reasoning.
- [ ] S16 resolved: the recall figure that fires the pivot, written down.
- [ ] S7 resolved: model floor measured across all seven metrics, reported as a
      configuration, check-recovery rate primary.
- [ ] S17 resolved: llguidance embedded, inference path chosen in an ADR, license
      verified, compatibility matrix published.
- [ ] The grounding contract is structural — `Verdict` is unforgeable outside
      `trellis-checks`, and `ToolResult` has no pass variant.
- [ ] The loop is one main loop with tools, not a verification fan-out, and the
      ADR says why.
- [ ] Multi-agent mode parallelizes work only — no agent produces a verdict on
      another agent's output, and the ADR says why.
- [ ] Partitioning treats unresolved and candidate edges as unpartitionable
      (**S19**), and integrated checks are never skipped on the strength of
      partition-local passes.
- [ ] `trellis doctor` reports backend batching capability and the multi-agent
      support tier (**S18**).
- [ ] Rollback verified by induced mid-run failure in CI.
- [ ] Bidirectionality verified by property test over `forward`/`reverse`.
- [ ] The v1.0 gate appears verbatim in `ROADMAP.md`, all five conditions.
- [ ] v1.0 non-goals explicit and substantial.
- [ ] Every major technology choice has a recorded rejected alternative.
- [ ] Factual claims cited; unverifiable ones flagged in `OPEN_QUESTIONS.md`.
- [ ] No marketing language anywhere.
