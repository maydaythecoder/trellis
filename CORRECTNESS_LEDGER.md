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
| Tests | **375** passing, 0 failed, 0 ignored (328 at the baseline commit) |
| Line coverage | **82.74%** (regions 82.52%, functions 86.34%) |
| Mutation score | **partial** — 842 of ~1,253 mutants; see Tier 6 before quoting any figure |

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

**Finding C-004 — the fix for C-003 shipped a defect of its own. Fixed,
`ec2463c`.** Changing what `Interner` serialises changed the shape of
`.trellis/index.bin`, and `FORMAT_VERSION` — the constant that exists for
exactly this — was not bumped. Postcard is not self-describing, so nothing in
an old blob says which layout wrote it.

The commit that introduced it asserted the opposite: *"old cache files no
longer parse; `load` already treats that as a miss."* That was not verified,
and the stated mechanism is wrong — `load` deserialises the whole blob
*before* comparing `version`, so the constant cannot rescue a payload change
by itself. What makes the guard work is that `version` and `fingerprint` both
precede `index` in the struct, and that `FORMAT_VERSION` is hashed into the
fingerprint.

Worth keeping in the ledger rather than quietly fixing. The campaign's own
repair introduced a live defect, and it was caught by review rather than by
any of the eleven tiers: the test suite was green across the change, because
no test wrote a blob in one format and read it in another. The test that now
exists writes a blob that is well-formed under the current layout, carries the
correct fingerprint, and differs only in version — so it proves the guard
fires rather than a corrupt decode failing in its place.

## Tier 6 — Mutation

**Running again, at `486fff1`, on request.** The first attempt was stopped at
842 of ~1,253 mutants — Claude Code killed the background job because the
system was low on memory while the session was idle, which was not a failure
of the run. Its raw output is kept in `docs/bench/mutants-94c285c/` so the
triage below can be checked against the list it came from.

Two things about the restart are deliberate and change what the result means:

- **`-j 2`, not `-j 4`.** Memory was the failure mode, so halving the
  concurrent test processes is the change most likely to let it finish. It
  roughly doubles the wall time.
- **At `486fff1`, not at `94c285c`.** Everything closed since the first
  attempt — the CLI suite, the approval-view tests, the binding forms — landed
  in between. Re-running the old commit would measure a state that no longer
  exists; running HEAD produces the score the quality audit's §9 rule actually
  needs. The figures below stay as the record of where the campaign started,
  and the two are **not** comparable as a before/after, because the second run
  covers four crates the first never reached.

What the stopped run established, against `94c285c`:

```
tested 842 of ~1253      caught 421 · missed 320 · unviable 93 · timeout 3

crate              caught  missed    score
trellis-checks         31       2    93.9%
trellis-cli           111     206    35.0%
trellis-core            5       3    62.5%
trellis-index         274     109    71.5%
                      421     320    56.8%

never reached:  trellis-loop · trellis-model · trellis-trail · trellis-vcs
```

**56.8% is not a workspace score and must not be quoted as one.** `--no-shuffle`
walks packages in a fixed order, so the 411 untested mutants are not a random
remainder — they are four whole crates, and those four carry the highest line
coverage in the repository (`agent.rs` 96.3%, `tier.rs` 96.8%, `trail/lib.rs`
96.6%, `wire.rs` 100%). The partial figure is therefore biased **low**, by an
amount this run cannot measure.

It is also **pre-C-001**: `trellis-cli` at 35.0% is the state before
`crates/trellis-cli/tests/cli.rs` existed, and 108 of its 206 misses are in
`main.rs`.

### Triage of the 320 that survived

Four buckets, by what each says.

**1. Already addressed — `main.rs`, 108 mutants.** `replace cmd_log -> Result<()>
with Ok(())` and its siblings. These are C-001 exactly, and the CLI suite
landed after this baseline was taken. A re-run is the only thing that can say
by how much.

**2. A real gap in the newest code — `radius.rs`, 35 mutants.** The largest
cluster sits on the functions ADR 0016 added: `doubt_in_files`,
`callers_of_files`, `file_ids`, `FileDoubt::{one_line, unexamined}`.

> `replace doubt_in_files -> FileDoubt with Default::default()` survives.

That one matters beyond arithmetic. `FileDoubt` is the doubt half of the two
facts ADR 0002 and ADR 0014 require every radius to ship with, and a default
`FileDoubt` reports **zero doubt** — silent confidence about files nothing is
known about, which is the precise failure Constraint 1 exists to prevent. The
narrowing shipped with `examples/narrowing_safety.rs` measuring its safety and
with no unit test on the functions themselves.

**3. Python syntax the walker handles and no test writes — `parse.rs`, 15.**
Deleted match arms survive for `relative_import`, `lambda`, `as_pattern`,
`named_expression`, and the splat parameter patterns. Each is a real form the
corpus contains and the suite does not.

Buckets 2 and 3 are closed: ten tests on the approval-time view
and six on the binding forms. The `doubt_in_files` one is stated as its own
test, because `FileDoubt::default()` is a legitimate value meaning *nothing is
in doubt* and the defect was that it was reachable by accident from a file
that had doubt in it.

Two of the binding-form tests were written and then deleted: `relative_import`
was already covered by `relative_import_resolves` and
`parent_relative_import_resolves`, which the surviving mutant did not imply.
A mutant surviving says a *behaviour* is unasserted, not that a function is
untested — the deleted arm there must be reachable by some other path.

**4. Counters and accessors — the remainder.** `+=` becoming `*=` in
`Coverage::merge` and `RadiusCoverage::record_ambiguous`; `site_count`,
`symbol_count`, `file_count` returning 0 or 1; `Reason::explain` returning an
empty string. Low severity individually. `Reason::explain` is the one worth
noting: it is what the CLI prints to say *why* an edge is uncertain, so an
empty return leaves the output shaped correctly and saying nothing.

### The three timeouts

```
tui/mod.rs:181       replace spawn_run with ()
incremental.rs:118   replace += with *= in input_edit
incremental.rs:124   replace += with *= in input_edit
```

These look like genuine consequences of the mutation rather than load
artifacts — a TUI waiting forever for a run that was never spawned, and byte
offsets multiplied instead of advanced being fed to tree-sitter. **Not
confirmed**, because confirming means re-running them and the run is stopped.

## Tier 2b — Truncating casts, examined

Raised by the quality audit's Pass 7, which found `cast_possible_truncation`
to be the largest pedantic category at 47 hits. Casts are not a readability
signal, so Pass 7 left them; they are a correctness question and belong here.

**Examined, and clear.** All 47 are `usize -> u32` or `usize -> u64` on the
length of a collection derived from a Python repository: symbols, files, call
sites, edges, the number of leading dots in a relative import. Truncation
needs a count above `u32::MAX`, and the one arena that could plausibly grow
already refuses first —

```rust
let sym = Sym(u32::try_from(self.store.len()).expect("interner exhausted"));
```

— and every symbol interns a name, so the symbol count cannot outrun it.

One theoretical case, named rather than fixed. `tui/view.rs` computes a cursor
column as `(app.input.chars().count() as u16).min(inner.width...)`: the cast
happens *before* the clamp, so an input line longer than 65,535 characters
would wrap to a small column instead of clamping to the pane width. It is a
cursor position in a TUI, it requires typing 65k characters into one line, and
it corrupts nothing. Recorded so that the next reader does not have to
re-derive that it is harmless.

This is a negative result and is kept on purpose. A category examined and
found clear is worth writing down; the alternative is examining it again.

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
| C-004 | — | The C-003 fix changed the cache's serialised shape without bumping `FORMAT_VERSION`; the commit's claim about it was unverified and its stated mechanism wrong | **Fixed**, `ec2463c` — found by review, not by any tier |
| C-005 | 3 | Three cache fixtures still used fixed temp paths — the last of the collision class from `c526c83` | **Fixed**, `ec2463c` |
| C-006 | 3 | Two of the new CLI assertions were vacuous: one passed on `"no tests ran"` matching `"no "` while the tools were in fact installed and running; the other checked for two phrases `explain` never emits | **Fixed**, `ec2463c` — both now written against output that was read |
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
