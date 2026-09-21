# Correctness campaign ledger

The campaign's own record. Every tier says what was run, what it found, and —
where a tier was skipped — why skipping it is defensible rather than
convenient.

| | |
|---|---|
| Repository root | `C:\Users\mayda\Documents\dev\trellis` |
| Baseline commit | **`94c285c`** |
| Toolchain | rustc 1.93.0 (254b59607 2026-01-19), pinned by `rust-toolchain.toml` |
| Tools | cargo-mutants 27.1.0 · cargo-llvm-cov 0.9.1 · cargo-audit 0.22.2 · cargo-deny 0.20.2 |
| Tests | **352** passing, 0 failed, 0 ignored (328 at the baseline commit) |
| Line coverage | **82.74%** (regions 82.52%, functions 86.34%) |
| Mutation score | **running** — see Tier 6 |

Every figure here is against `94c285c`. A score quoted without its commit is
not a baseline, because the next commit moves it.

## Tier 0 — Baseline

Was **not met** when the campaign opened. Two things were missing and both are
now in place:

- **No pinned toolchain.** Mutation scores and coverage percentages are
  toolchain-sensitive, and a `trybuild` compile-fail proof is *extremely*
  toolchain-sensitive — the `Verdict` seal's expected stderr had already broken
  once because rustc 1.93 emits a macro backtrace note that 1.95 does not.
  Fixed: `rust-toolchain.toml` pins `1.93.0`.
- **No ledger.** This file.

## Tier 1 — Compilation

Clean.

```
cargo check --workspace --all-targets --all-features
```

No warnings. `unsafe_code = "forbid"` at workspace level, so Tier 8 is
structurally satisfied rather than argued.

## Tier 2 — Lints

Clean, including `clippy::correctness` and `clippy::suspicious`.

```
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

0 warnings.

## Tier 3 — Tests

328 passing, 0 ignored. One defect found and fixed *by this tier*, and it is
the kind a green suite hides:

> **Test fixtures collided between concurrent processes** (`c526c83`). Five
> fixtures used fixed temporary paths and did `remove_dir_all` then
> `create_dir_all`. Run alone: 328/328 green. Run under `cargo mutants -j 4`,
> which starts four test processes at once: **9 failures** across
> `doctor_tests`, `init_tests` and `interrupt_tests`, as each process deleted
> the others' repositories.
>
> This mattered beyond tidiness. It would have made every mutation result
> untrustworthy — a "caught" mutant is indistinguishable from a fixture that
> was deleted mid-run. Fixed by appending `std::process::id()`; proved by
> running two full `trellis-cli` suites concurrently, 0 failures where there
> had been 9.

The first mutation run was discarded because of this and restarted.

## Tier 4 — Manual review

Partial, and honestly labelled. Most of this code was written in the sessions
now reviewing it; see QUALITY_LEDGER.md's *caveat about independence*. The
findings that came out of reading rather than tooling are in that file.

## Tier 5 — Coverage

**Run.** `docs/bench/coverage-20260921.md` holds the full table.

```
TOTAL                     9590 lines   1655 missed   82.74%
main.rs alone             1361 lines    829 missed   39.09%   = 50.1% of all missed
tui/ (5 files)             632 lines    156 missed   75.32%   =  9.4% of all missed
everything else           7597 lines    670 missed   91.18%
```

**Finding C-001 — the CLI command layer had no test at all. Fixed, `0ba3b5f`.**
`crates/trellis-cli` had no `tests/` directory. Its three `#[cfg(test)]`
modules test extracted helpers; every `cmd_*` entry point is called from
exactly one place, the dispatch in `main`, and from no test. Ten commands,
829 uncovered lines.

`crates/trellis-cli/tests/cli.rs` now drives the real binary through
`CARGO_BIN_EXE_trellis` — twenty tests, asserting mostly *what the output may
not say*, because both project constraints are negative. It found a defect on
its first run: `main.rs` carried a comment saying `trellis model` must work
outside a repository, models being user state rather than repository state,
while `needs_repo` exempted only `init`. The comment was right and the code
did not do it.

The library crates sit at 91.18%. The remaining gap is a TTY-bound TUI shell
(deliberate — `model.rs` and `view.rs` were factored out precisely so the
testable part could be tested, and they are at 97% and 90%) and a transport
layer that needs a live runtime.

**Finding C-002 — `trellis_vcs::Worktree` is dead code.** `create` and
`discard`, written ahead of need for brief 4.13's per-partition isolation,
referenced from nowhere. Not merely uncovered — unreferenced. Reported rather
than deleted: v1.1 plans to use them and the question is sequencing.

## Tier 6 — Mutation

**Running.** `cargo mutants --workspace --no-shuffle -j 4` against `94c285c`,
~1,253 mutants.

At 153 mutants tested the shape is already unambiguous and it agrees with
Tier 5 exactly:

```
caught 45 · missed 85 · unviable 18 · timeout 0
missed by file:  main.rs 85   coverage.rs 2
```

**85 of 87 missed mutants are in the one file with no tests.** Mutants like
`replace cmd_log -> Result<()> with Ok(())` and `replace discover_root ->
Result<PathBuf> with Ok(Default::default())` survive because nothing ever
calls those functions outside the dispatch in `main`.

This is why C-001 is the campaign's highest-leverage finding rather than a
tidiness note: the quality audit's standing rule is *"mutation score must not
drop"*, and a score cannot drop from zero. Its Pass 5 intends to split
`main.rs` by command — exactly the refactor that rule guards — and the guard
is blind there today.

Final score, the full missed-mutant triage, and the delta after C-001 is
fixed go here when the run lands.

## Tier 7 — Property and fuzz

Partial. Property tests exist (`partition_invariants`, `bidirectional`); there
are no fuzz targets. The parser is the obvious fuzz candidate and it is
tree-sitter's, not ours — a fuzzer there would test tree-sitter. The
first-party candidate is `UntrustedTrail::from_json`, which by construction
parses attacker-influenced input; noted, not built.

## Tier 8 — Undefined behaviour

**N/A by construction.** `unsafe_code = "forbid"` at workspace level. No
`unsafe` block exists to audit, and one cannot be added without changing the
lint. Miri is therefore not informative here.

## Tier 9 — Concurrency

**Run, by audit rather than by tool**, because the result of the audit is that
there is very little for a tool to find.

**There is no shared mutable state.** A workspace-wide search for `Mutex`,
`RwLock`, `Atomic`, `Arc<`, `OnceLock`, `LazyLock` and `thread_local` across
`crates/*/src` returns nothing but two comments saying so. The orchestrator
owns the `Index` outright; that is the design, and it is what makes this tier
short.

What concurrency exists, and what was checked:

| Site | Shape | Verdict |
|---|---|---|
| `orchestrator.rs:183` | `mpsc::channel(32)` + oneshot replies, one actor task | Bounded and deliberate — an unbounded queue would be an invisible second budget. `serve` handles each request by calling `self.*` directly and never sends into its own inbox, so the bounded channel cannot deadlock on itself. |
| `orchestrator.rs:189` `serve` | Strictly sequential | `run_checks` awaits subprocesses while holding the actor. A throughput property today, and a **v1.1 hazard**: with N agents on one orchestrator, one long check serialises everyone. Recorded before it bites. |
| `tui/events.rs:63` | `std::sync::mpsc`, two detached threads | Both exit on channel disconnect (`send(..).is_err()`). Correct. One latent wrinkle: the input thread blocks in `event::read()`, so it exits at the *next* keystroke, not at drop — harmless while `tui::run` is called once per process, which it is. |
| `main.rs:826,1648,2109`, `session.rs:329` | `Runtime::new()` + `block_on` | All four sit in **sync** functions reached from a **sync** `fn main` — there is no `#[tokio::main]` in the tree. No nested-runtime panic is reachable. |
| `resolve.rs:475`, `incremental.rs:57` | `rayon` `par_iter().map().collect()` | Order-preserving by rayon's contract, and `discover` sorts, so `FileId`/`SymbolId` assignment is positional and deterministic. **Untested** — see C-003. |

**Finding C-003 — index determinism was relied on and never asserted. Fixed,
`0ba3b5f`, and the test found something.** The graph *is* deterministic: every
field of two builds over identical sources compares equal. Its serialised
bytes were not. `Interner` held `lookup: HashMap<Box<str>, Sym>` — a second
copy of every key in the `store` vector beside it — and serialised it, and a
`HashMap` iterates in an order that differs between instances holding
identical contents.

So `.trellis/index.bin` differed on every rebuild of unchanged sources. Not a
correctness fault (the cache is fingerprint-keyed and round-trips fine) but a
file that changes when nothing changed cannot be compared, hashed, or used as
evidence that nothing moved — which is an odd property for this project to
have shipped.

The interner now serialises as the store alone and rebuilds `lookup` on load.
That also makes the invariant *lookup agrees with store* hold by construction,
where it was previously taken on trust from a file on disk.

```
poetry 2.5.0, 3,735 symbols:   index.bin  586.9 KiB -> 492.6 KiB   (-16.1%)
                               rebuild    differed every time -> byte-identical
```

The first version of the test would have missed this. It compared
`serde_json::to_value`, which normalises map key order and reported the two
builds identical — the exact defect it was written to catch. It compares text
now, and a second test asserts byte-reproducibility outright.

## Tier 10 — Dependencies

**Run and clean.**

```
cargo audit       →  no vulnerabilities (1 documented unmaintained transitive)
cargo deny check  →  advisories ok, bans ok, licenses ok, sources ok
```

`cargo deny` had never passed before this campaign, for a purely procedural
reason: with no policy file nothing is allowed, so every MIT crate in the tree
was rejected. `deny.toml` now states the policy, with every licence expression
read out of the lockfile rather than copied from a template.

One advisory is ignored, with reasoning recorded in `deny.toml` and a stated
condition under which it stops being acceptable: `RUSTSEC-2023-0089`,
`atomic-polyfill` unmaintained, reached through `heapless 0.7 <- postcard`.
Unmaintained rather than vulnerable, and `cargo tree -i atomic-polyfill` on
the host target prints nothing — it is gated behind targets without native
atomics and is compiled for neither supported platform.

The crates are also now `publish = false`, which is true (the deliverable is
one binary, not eight libraries) and prevents an accidental `cargo publish`.

## Tier 11 — Formal methods

N/A. Nothing here is a protocol or a data-structure invariant that would repay
a proof. The one place the project does reach for a machine-checked guarantee
is the `Verdict` seal, and the check is a `trybuild` compile-fail test — the
type system as the proof obligation, which is ADR 0004's whole point.

## Findings

| ID | Tier | Finding | State |
|----|------|---------|-------|
| C-001 | 5, 6 | `trellis-cli` had no `tests/`; ten `cmd_*` entry points untested; 829 uncovered lines and 85 of the first 87 missed mutants | **Fixed**, `0ba3b5f` — 20 tests, and it caught `trellis model` refusing to run outside a repository against its own stated intent |
| C-002 | 5 | `trellis_vcs::Worktree::{create,discard}` is unreferenced dead code | Reported |
| C-003 | 9 | Index determinism under rayon is depended on by the cache and never asserted | **Fixed**, `0ba3b5f` — the graph was deterministic, its bytes were not; interner no longer serialises a derivable map |
| — | 3 | Test fixtures collided between concurrent processes | **Fixed**, `c526c83` |
| — | 0 | Toolchain unpinned | **Fixed**, `94c285c` |
| — | 10 | No dependency policy; `cargo deny` could not pass | **Fixed**, `bc705e1` |

## What this campaign does not establish

The tiers that ran are the tiers that can run on this machine. Specifically:

- No tier exercised the model transport, because that needs a runtime at or
  above the capability floor and there isn't one. `adapter.rs` at 61.90%
  function coverage is that gap, measured.
- Tier 4 is not an independent review and does not claim to be.
- A mutation score is a statement about the tests, not about the code. A high
  score on a suite that asserts the wrong thing is worth nothing, and nothing
  here checks that the assertions are the right ones.
