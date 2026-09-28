# CLAUDE.md

Conventions for this repository. Read before changing anything.

## What this is

TRELLIS: a Rust agent harness for Python repositories. It builds a call graph of
first-party code, drives an agent loop against it, verifies changes with the
repo's own type checker and test suite, and commits a decision trail.

`docs/BRIEF.md` is the specification. `docs/MISSION.md` is the short version.

## Commands

```
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets
cargo fmt --all

cargo run -p trellis-cli -- index          # full index: parse, resolve, coverage
cargo run -p trellis-cli -- radius <sym>   # blast radius with confidence breakdown
cargo run -p trellis-cli -- check          # run the grounded checks
cargo run -p trellis-cli -- doctor         # probe tier, batching, tooling

# owner-directed growth beyond brief 4.12, each recorded in its ADR
cargo run -p trellis-cli -- run "<task>" --focus <stem>    # vault note as context (ADR 0017)
cargo run -p trellis-cli -- run "<task>" --approve|--auto  # the control dial (ADR 0018)
cargo run -p trellis-cli -- queue add "<task>" [--focus <stem>]   # (ADR 0020)
cargo run -p trellis-cli -- work [--once | --every <s>] [--max-tasks N]
cargo run -p trellis-cli -- vault check|search|graph|new|reparent|link|retitle|rm|init|dates   # (ADR 0022)
cargo run -p trellis-cli -- trust [--show | --revoke]      # hooks, custom tools, bash (ADR 0023)
cargo run -p trellis-cli -- run "<task>" --model anthropic:https://api.anthropic.com#<model>  # hosted (ADR 0024)
cargo run -p trellis-cli -- run "<task>" --json            # JSONL on stdout (ADR 0025)
cargo run -p trellis-cli -- rpc                            # JSONL commands on stdin (ADR 0025)
cargo run -p trellis-cli -- agents list | routines list    # profiles, routines (ADR 0026)
cargo build -p trellis-cli --no-default-features           # local-only binary, no TLS (ADR 0024)

# ADR 0021 is the map: every pi, Apex and Paperclip capability, and whether
# it is built, deferred, or excluded (and by which clause).

# qa/run-guards.py runs whichever `trellis` is on PATH, and dirty_worktree
# needs the demo's runtime up: put target/debug first and start the runtime,
# or the guards test an old binary and one refuses for the wrong reason.

# v0.1 measurement tooling (examples, not CLI surface — the surface is fixed
# by brief 4.12 and does not grow to suit us; growth the owner directs is
# recorded in an ADR, as 0017, 0018, 0020 and 0021-0026 do)
cargo run --release -p trellis-index --example audit_sample -- <root> <n> <seed> <dir>
cargo run --release -p trellis-index --example audit_score  -- <dir>
```

Agent-driven work goes through the gates in `scripts/gates/`
(`scripts/gates/run-all.sh [<issue>]`) and the orchestrator
(`claude --agent orchestrator`); see `docs/BUILD_SYSTEM.md`. A ticket is done
when every gate exits 0, not when an agent says so.

`trellis run`, `resume` and `partitions` are implemented. They were stubs
until the agent loop landed in v0.2, and the rule that kept them stubs still
holds for anything added next: a command that appears to work without doing
the work is worse than one that says it is not built yet.

## The number that is easy to misread

`trellis index` prints what the resolver *bound*. That is a **lower bound on
recall, not recall**, and it must never be compared to the thresholds in
`BENCHMARK_AND_EVAL.md` §3. An `obj.method()` whose receiver type is unknown is
counted as a miss even when its true callee is third-party, which §2 excludes
from the denominator outright. Only the hand-audited sample produces a
comparable figure. See A-30.

That figure moved from 37.6% to 43.7% on poetry when ADR 0015 corrected which
sites belong in the denominator. **`resolved` did not change**, hand-audited
recall did not change, and no additional call site became resolvable. Any
before/after of this number must state both halves or it reads as an
improvement that did not happen.

## Receiver typing, and why it is allowed to be wrong in only one direction

`trellis-index` types receivers from annotations and constructor assignments
(ADR 0012). Every rule there can only *add* resolutions, so every one is a new
chance to emit an unsound edge. Three invariants hold it in place, and all
three earned their place by catching a real defect:

- A **declared type is an upper bound, not an identity.** If any first-party
  subclass overrides the method, the result is `Candidates` over the base and
  every override. Deliberately not applied to `self.m()`, where the audit
  protocol resolves to the enclosing class.
- A type fact is honoured **only when it resolves to a first-party class**, so
  `x = make_env()` types nothing.
- **Conflicting statements type nothing**, and `list[Env]` types `list`.

Before changing any of them, run the audit: `examples/check_soundness.rs` over
the corpus must stay at zero violations, and precision on the audited sample
must stay at 100%.

## Crate boundaries are load-bearing

```
trellis-core     shared ids, interner, errors      (depends on nothing)
trellis-index    tree-sitter -> call graph         (must NEVER depend on trellis-model)
trellis-checks   typecheck/tests/imports           (owns Verdict; the only crate that may assert correctness)
trellis-model    adapter tiers, llguidance
trellis-loop     agent loop, tools, budgets, partitioning
trellis-trail    the decision trail
trellis-vcs      git snapshot and rollback
trellis-vault    Apex vault -> one note's context  (must NEVER depend on trellis-model)
trellis-queue    local task queue + heartbeat loop (file-backed, in the git dir; no trellis-loop/model)
trellis-cli      the binary
```

Two of these are not stylistic:

- **`trellis-index` must not depend on `trellis-model`.** The graph has to be
  testable with no model anywhere near it. A graph whose output depends on a
  sampler is not a graph. The same holds for `trellis-vault`: what the model
  is shown is a pure function of files on disk, asserted byte for byte against
  committed goldens (ADR 0017).
- **`trellis-checks` owns `Verdict`.** That is what makes the grounding contract
  structural rather than advisory.

## Invariants the PR #1 review rounds put in place

Each one is held by a test that names it. Read the ADR before loosening any
of them.

- **Every `git` spawn goes through `trellis_vcs` (`command()`/`git_argv`)**,
  so a `core.fsmonitor` or hooks path planted in `.git/config` never runs
  under trellis. `tests/hygiene.rs` fails on a bare spawn. `chat` keeps a
  copy of the overrides, and a test ties that copy to `trellis_vcs`.
  (ADR 0023)
- **Every child process drops the provider-key variables**
  (`trellis_core::secrets::scrub`). `tests/child_env.rs` fails on an
  unscrubbed spawn. A process agent gets back only the variables its profile
  names in `pass_env`. (ADR 0024 §3, 0026)
- **Repository-supplied prompt inputs are read through
  `trellis_core::paths::resolve_repo_input`.** Nothing that resolves outside
  the repository or into `.git` reaches a prompt. (ADR 0025)
- **Only the user decides where a key is sent.** A repository may not name a
  `key_env`, and a hosted or loopback endpoint the repository chose is not
  given a key unless the user's own config or `--model` names it.
  (ADR 0024 §3)
- **Model writes never land in `.trellis/` or `.git/`, and trusted code
  lives under `.trellis/`.** A hook or custom tool that names repository code
  elsewhere is refused at `trellis trust`. (ADR 0023)
- **A pass stops counting once the tree changes.** So does a run whose HEAD
  moved, or whose trusted code or `.git/` changed during the run.
  (ADR 0004, 0009, 0023)
- **Rollback is complete only for trellis's own writes.** With `bash`,
  hooks, custom tools or process agents enabled, it is best-effort, and the
  ADRs list the known ways around it. Do not add another post-hoc git check
  to close one more of those; the fix is a sandbox (ADR 0021, row 13.3).

## Rules that need an ADR to change

Changing any of these without a new ADR should fail review regardless of how
good the reason sounds.

1. **`Verdict` is unforgeable outside `trellis-checks`.** Private `passed`,
   private `Seal`, three `pub(crate)` constructors each taking a
   `ProcessOutput`. No `Deserialize` — a derived one is a public constructor
   from arbitrary JSON. → ADR 0004
2. **`ToolResult` has no pass variant.** The model never issues a correctness
   verdict. → ADR 0004
3. **Every blast radius ships with its coverage, as two facts and never a
   ratio.** No API returns reach without limits; nothing emits a bare
   "complete"; and `RadiusCoverage` has no `total()` and no fraction, because
   its reach counts and its doubt count come from different populations.
   Dividing them printed "3.7% resolved" for a resolver audited at 72%.
   → ADR 0002, ADR 0014
4. **`Verified` and `Reviewed` are never collapsed into one number.**
   `ModeCounts` has no `total()` on purpose. → ADR 0003
5. **No agent produces a verdict on another agent's output.** Agents
   parallelize work, never judgment. → ADR 0005
6. **Unresolved and candidate edges are unpartitionable**, and integrated checks
   are never skipped on the strength of partition-local passes. → ADR 0005, S19
7. **A clean worktree is required before a run.** → ADR 0009
8. **The user's API key never touches infrastructure we control.** → ADR 0010
9. **Budgets divide across partitions; they never multiply.** → ADR 0008
10. **Trails read back as untrusted data.** → ADR 0007

## Style

- `thiserror` in libraries, `anyhow` in the binary.
- `tracing` for instrumentation, and it doubles as the budget accounting
  substrate. Do not build a second telemetry path.
- Comments explain **why**, not what. A comment restating the code is noise; a
  comment naming the constraint a line satisfies is the point.
- Tests assert behaviour and name the risk they defend where one applies — see
  `s19_units_reached_through_uncertain_edges_share_one_conservative_partition`.
- No marketing language, in code, comments, docs, or CLI output. State what is
  true and what is not known.

## Versions

`tree-sitter` and `tree-sitter-python` share a C ABI. **Do not bump them
independently.** The verified pairing is 0.27.0 / 0.25.0, asserted by
`crates/trellis-index/tests/grammar_abi.rs`, which turns a runtime failure into
a test failure.

`bincode` 3.0.0 is a protest release that deliberately fails to compile. The
workspace uses `postcard`.

## Dependency pinning

- Requirements in `Cargo.toml` use Cargo's default caret form (`"1.2"`), not
  exact `=` pins.
- `Cargo.lock` is committed, and it is the pin: CI and every contributor
  build the versions it records.
- A lockfile update is its own commit, and `cargo deny check` and
  `cargo audit` both pass on it. CI runs both on every push and pull request
  (the `supply-chain` job).
- `tree-sitter` and `tree-sitter-python` move together; see Versions above.
- A new crate dependency still needs an ADR.

## Before claiming a number

No measurement goes into a document without its method beside it. If it cannot
be measured yet, write "pending" and name the milestone. The documents in
`docs/` currently contain no fabricated figures, and that property is worth more
than any individual number in them.
