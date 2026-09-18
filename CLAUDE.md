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

cargo run -p trellis-cli -- index          # Pass 1 over the repo, with counts
cargo run -p trellis-cli -- check          # run the grounded checks
cargo run -p trellis-cli -- doctor         # probe tier, batching, tooling
```

`trellis radius`, `run`, `resume` and `partitions` exist in the CLI surface and
exit with a clear "not implemented yet" — they need the resolver and the agent
loop (v0.2). Do not stub them into something that appears to work.

## Crate boundaries are load-bearing

```
trellis-core     shared ids, interner, errors      (depends on nothing)
trellis-index    tree-sitter -> call graph         (must NEVER depend on trellis-model)
trellis-checks   typecheck/tests/imports           (owns Verdict; the only crate that may assert correctness)
trellis-model    adapter tiers, llguidance
trellis-loop     agent loop, tools, budgets, partitioning
trellis-trail    the decision trail
trellis-vcs      git snapshot and rollback
trellis-cli      the binary
```

Two of these are not stylistic:

- **`trellis-index` must not depend on `trellis-model`.** The graph has to be
  testable with no model anywhere near it. A graph whose output depends on a
  sampler is not a graph.
- **`trellis-checks` owns `Verdict`.** That is what makes the grounding contract
  structural rather than advisory.

## Rules that need an ADR to change

Changing any of these without a new ADR should fail review regardless of how
good the reason sounds.

1. **`Verdict` is unforgeable outside `trellis-checks`.** Private `passed`,
   private `Seal`, three `pub(crate)` constructors each taking a
   `ProcessOutput`. No `Deserialize` — a derived one is a public constructor
   from arbitrary JSON. → ADR 0004
2. **`ToolResult` has no pass variant.** The model never issues a correctness
   verdict. → ADR 0004
3. **Every blast radius ships with its coverage.** No API returns reach without
   limits; nothing emits a bare "complete". → ADR 0002
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

## Before claiming a number

No measurement goes into a document without its method beside it. If it cannot
be measured yet, write "pending" and name the milestone. The documents in
`docs/` currently contain no fabricated figures, and that property is worth more
than any individual number in them.
