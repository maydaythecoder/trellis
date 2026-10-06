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
cargo run -p trellis-tickets -- new --title <t> --body-file <p> --dry-run   # file a ticket (ADR 0040)
```

Every other command, with the ADR that added it, is in `docs/COMMANDS.md`.
The CLI surface is fixed by brief 4.12 and grows only by an owner-directed
ADR. `qa/run-guards.py` runs whichever `trellis` is on PATH: put
`target/debug` first. The resolver's measurement rules (the recall number,
receiver typing) load from `.claude/rules/index-soundness.md` whenever
`crates/trellis-index/` is touched.

Agent-driven work goes through the gates in `scripts/gates/`
(`scripts/gates/run-all.sh [<issue>]`) and the orchestrator
(`claude --agent orchestrator`); see `docs/BUILD_SYSTEM.md`. A ticket is done
when every gate exits 0, not when an agent says so.

`trellis run`, `resume` and `partitions` are implemented. They were stubs
until the agent loop landed in v0.2, and the rule that kept them stubs still
holds for anything added next: a command that appears to work without doing
the work is worse than one that says it is not built yet.

## Branches and host

- `main` is the launch branch, `dev` the trunk, `audit` the integration
  branch (ADR 0051). Cut `t<n>-<slug>` or `feature/<slug>` from `origin/dev`,
  merge a green branch into `audit`, promote `audit` into `dev`; `dev` reaches
  `main` only by the owner's pull request, squash-merged. Never push `main`,
  `v0.1-resolver`, `drain` or a `v*` tag.
- Run gates from Git Bash, not WSL. One cargo process at a time: the host has
  15 GB shared with a local model, and parallel builds have been killed for
  memory. G2b needs the local runtime listening on 127.0.0.1:11434.

## Context and vague requests

- Search before reading (Grep, Glob), then read by section with an offset and
  limit. A sweep wider than about five files goes to a subagent that returns
  conclusions and `file:line`, not file contents.
- Restate a vague request in one line (goal, area, what done looks like) and
  resolve it from the repository first: ADRs, open issues, `docs/plans/`,
  `git log`. Ask only what changes the work or touches a key, a deletion or an
  owner decision, with a recommended answer; otherwise take the conventional
  choice and say so in the report.

## Crate boundaries are load-bearing

```
trellis-core     shared ids, interner              (depends on nothing)
trellis-index    tree-sitter -> call graph         (must NEVER depend on trellis-model)
trellis-checks   typecheck/tests/imports           (owns Verdict; the only crate that may assert correctness)
trellis-model    adapter tiers, llguidance
trellis-loop     agent loop, tools, budgets, partitioning
trellis-trail    the decision trail
trellis-vcs      git snapshot and rollback
trellis-vault    Apex vault -> one note's context  (must NEVER depend on trellis-model)
trellis-queue    local task queue + heartbeat loop (file-backed, in the git dir; no trellis-loop/model)
trellis-tickets  ticketing standard's label rules    (pure functions + a separate binary; depends only on trellis-core; ADR 0040)
trellis-cli      the binary
tui-demo         the TUI against a mock event source (depends on no trellis crate; trellis-cli takes it with default-features = false)
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
  under trellis. `tests/hygiene.rs` fails on a bare spawn. (ADR 0023)
- **Every child process drops the provider-key variables**
  (`trellis_core::secrets::scrub`). `tests/child_env.rs` fails on an
  unscrubbed spawn. A process agent gets back only the variables its profile
  names in `pass_env`. (ADR 0024 §3, 0026)
- **Repository-supplied prompt inputs are read through
  `trellis_core::paths::resolve_repo_input`.** Nothing that resolves outside
  the repository or into `.git` reaches a prompt. (ADR 0025)
- **Only the user decides where a key is sent.** A repository may not name a
  `key_env`, and a hosted or loopback endpoint the repository chose is not
  given a key unless the user's own config or `--model` names it. (ADR 0024 §3)
- **Model writes never land in `.trellis/` or `.git/`, and trusted code
  lives under `.trellis/`.** A hook or custom tool that names repository code
  elsewhere is refused at `trellis trust`. (ADR 0023) Nor do they land in
  the configured vault's notes root (ADR 0049), held at each writer by the
  vault_notes_root_ tests in trellis-loop and, for runs and process agents,
  by a_model_edit_into_the_vault_is_refused_and_named and
  a_process_agent_s_write_into_the_vault_is_refused_and_named in trellis-cli.
- **A pass stops counting once the tree changes.** So does a run whose HEAD
  moved, or whose trusted code or `.git/` changed during the run.
  (ADR 0004, 0009, 0023)
- **Rollback is complete only for trellis's own writes.** `bash`, custom tools,
  hooks and process agents run in a throwaway clone, and what they changed
  there comes back under the refusals every writer meets, so those are
  trellis's writes too (ADR 0032): tracked-file edits through the
  orchestrator's write path for the first three, and edits, new files and
  deletions through its own carry-back for a process agent. Against a command
  that reaches the repository by absolute path it is best-effort, and the ADRs
  list the known ways around it. Do not add another post-hoc git check to close
  one more of those; the fix is isolation (ADR 0032, "What is not built").

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
