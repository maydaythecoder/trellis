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
| Tests | **487** passing, 0 failed, 0 ignored (328 at the baseline commit, 415 when the campaign's tiers closed) |
| Line coverage | **82.74%** (regions 82.52%, functions 86.34%), at `94c285c` and not re-measured since |
| Mutation score | **all 8 crates**, as self-test scores; three re-measured with the full suite. `trellis-cli` completed last, at `4cefe4a`: **41.5%**, the worst in the workspace — see Tier 6 |

The coverage and mutation figures are against `94c285c` and `4cefe4a`
respectively. A score quoted without its commit is not a baseline, because
the next commit moves it — and the test count above is the only figure here
that tracks `HEAD`, because it is the one that changes every commit.

**The mutation figure is now stale in a known direction.** Of
`trellis-cli`'s 144 live survivors, **87 are now killed by a test**, 9 are
triaged as unexercisable on this machine (C-025), and 1 is an equivalent
mutant (C-024) — 97 accounted for, 47 untouched. So the real score at `HEAD`
is well above 41.5%. It is left as measured rather than estimated: the
re-run is what replaces it, and an estimate in this row would be exactly the
fabricated figure the rule at the end of CLAUDE.md forbids.

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

**Two attempts, both stopped for system memory pressure. There is still no
workspace mutation score**, and the quality audit's Pass 6 stays blocked on
it.

| | reached | of | at | concurrency |
|---|---|---|---|---|
| run 1 | 842 | ~1253 | `94c285c` | `-j 4` |
| run 2 | 295 | ~1253 | `486fff1` | `-j 2` |

Raw output for both is kept under `docs/bench/`, so every figure below can be
checked against the list it came from.

### `-j 2` was a deliberate change and it did not work

Run 2 halved the concurrency specifically to reduce the memory footprint that
stopped run 1. It got **a third as far**, at roughly twice the wall time per
mutant. Whatever drives the kill is not this job's concurrency.

Stated plainly because the reasoning was mine and the evidence is against it.
A third attempt should not reach for `-j 1`.

### What run 1 established, at `94c285c`

```
caught 421 · missed 320 · unviable 93 · timeout 3

trellis-checks    93.9%      trellis-cli    35.0%
trellis-core      62.5%      trellis-index  71.5%
                 — 56.8% over what ran

never reached: trellis-loop · trellis-model · trellis-trail · trellis-vcs
```

**56.8% is not a workspace score.** `--no-shuffle` walks packages in order, so
the untested remainder is four whole crates rather than a random sample — and
those four carry the highest line coverage in the repository, so the figure is
biased low by an amount neither run can measure.

### What run 2 established: one file, and it is the one that mattered

Run 2 reached only `trellis-checks` and part of `trellis-cli`, dying inside
`session.rs`. One file is complete in **both** runs and therefore comparable:

```
main.rs survivors    108  (94c285c, no CLI tests)
                  ->  88  (486fff1, with the CLI suite)

twenty mutants stopped surviving.  none started.
```

Percentages are deliberately not quoted: run 1's `caught.txt` was not
preserved per file, only its survivors were, so survivor counts are what the
evidence supports. That was an error in how run 1 was archived and run 2 keeps
all four files.

The twenty are the C-001 measurement. Seven `cmd_*` entry points could have
had their whole bodies replaced with `Ok(())` and the suite would have stayed
green; so could `main`. Two are the `!needs_repo` guards — the gate that let
`trellis model` refuse to run outside a repository against its own comment,
which the same suite found by reading.

**88 still survive in `main.rs`**, concentrated in the paths integration tests
do not reach: model pull, interrupt handling, output formatting. A first cut,
not a solved file. `docs/bench/mutants-486fff1/README.md` has the list.

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

## C-016 — trail integrity depended on a feature nothing declared

**Closed.** The finding is real; the first two versions of this entry
described it wrongly, and both are stated below because the method is what
this campaign is for.

### What is actually true

`Trail::compute_factual_sha256` hashed the `Factual` **struct**, which
serialises in declaration order. `UntrustedTrail::integrity` parses the file
and hashes `v["factual"]` — a `serde_json::Value`. Whether those agree
depends entirely on what `serde_json::Map` is:

| `serde_json/preserve_order` | `Map` is | reader emits | agrees with the struct? |
|---|---|---|---|
| **on** | `IndexMap` | insertion order = file order = declaration order | yes |
| **off** | `BTreeMap` | sorted | **no** |

It is **on**, because `schemars` enables it — a crate `trellis-trail` does not
depend on, pulled in by `trellis-loop` and `trellis-model` to derive
tool-call schemas. So trail tamper-evidence worked, by way of a transitive
feature flag turned on for an unrelated reason, declared nowhere and tested
by nothing. Drop `schemars`, or have it stop needing `preserve_order`, and
every trail ever written starts reading as `Altered`.

`compute_factual_sha256` now goes through `Value` as well, so both sides
share whatever `Map` is and agree either way. Verified in both builds: the
CLI (feature on) reports no integrity line for a freshly written trail, and
`cargo test -p trellis-trail` (feature off) passes the round trip, which it
did not before the change.

### The two wrong versions, and what produced each

**"Every trail reads as `Altered`."** Wrong. It came from probing with
`cargo run -p trellis-trail --example`, which builds a smaller dependency
graph where `preserve_order` is off — so the probe read with `BTreeMap`
semantics a file written with `IndexMap` ones. **The mismatch was in the
instrument, not the product.** The correct signal was available the whole
time: `trellis log` printed no integrity line, and I talked myself out of it.

**"Users don't see the `ALTERED` line."** Also wrong, and in the opposite
direction — asserted from a `head -8` that cut the output above where the
line would have appeared. Neither the claim nor its retraction was measured.

The lesson that generalises: **a probe built from a different dependency
graph is a different program.** Feature unification makes `cargo run -p X
--example` and the real binary disagree about library behaviour, silently,
and nothing warns.

## Tier 6 — final

**Seven of eight crates measured. `trellis-cli` never completed**, stopped
four times by system memory: twice as part of a whole-workspace run, once at
`-j 2`, and once as a per-crate run whose partial output was corrupt.

### Self-test scores — what a crate's own tests catch

```
trellis-checks   31/2    93.9%        trellis-trail    98/22   81.7%
trellis-model    29/6    82.9%        trellis-index   291/92   76.0%
trellis-loop    123/57   68.3%        trellis-vcs      16/11   59.3%
trellis-core      5/5    50.0%
                                      seven crates   593/195   75.3%
```

`cargo mutants -p X` runs only X's tests, so these understate by however much
a crate is exercised from outside itself. That is not a footnote — it is the
difference between a number and a different number:

```
crate            self-test    whole suite    delta
trellis-vcs         59.3%         92.6%      +33.3 pts
trellis-core        50.0%         70.0%      +20.0 pts
trellis-model       82.9%    not computable  5 timeouts, unusable by our own rule
```

`trellis-model`'s re-run is **not** reported as 83.8%. Thirty-seven verdicts
and five timeouts gives a range of [73.8%, 85.7%], and computing a percentage
over the verdicts alone is exactly the error the timeout guard exists to
prevent. Its five timeouts turned out to be our own bug — see C-016's
neighbour below — and it has not been re-run since that was fixed.

### What the triage produced

Twenty-four numbered findings in the table below, C-001 to C-025, plus three
that predate the numbering. C-007 is the only id in that range with no row:
it was the `usize as u32` truncation question, examined and found clear, and
is recorded as such in `QUALITY_LEDGER.md` rather than as a finding here —
an id that produced no defect still costs an id, because renumbering would
break every reference already written.

The ones that were defects rather than test gaps:

- **C-010** — a partition worktree would have dirtied the repository and made
  the *next* run refuse to start. Found by writing the first test of code
  that had none.
- **C-016** — trail tamper-evidence worked only because `schemars`
  transitively enables `serde_json/preserve_order`. Undeclared, untested, and
  one dependency change from every trail reading as `Altered`.
- **C-019** — the guard keeping a model's path inside the repository accepted
  `../../etc/passwd` under its surviving mutant, because that path is
  relative and so not absolute.
- **C-020** — S19's own rule, `radius disjoint && files disjoint`, would hand
  two agents the same file. It survived because it was only tested through
  `plan`, which cannot construct such a pair.
- The stub hang: an unbounded `accept()` meant a mutant that skipped the
  client's request hung the test rather than failing it. Five mutants the
  suite *does* catch were reported as timeouts, and in CI it would have
  stalled a build rather than reporting.

Four pieces of dead code were deleted and one tested rather than deleted,
because it had a named future use.

### What this tier does not establish

There is **no workspace mutation score**, and the per-crate figures are not
one. `trellis-cli` is unmeasured, and it is the largest crate and the one
with the most tests added during the campaign — so the number most likely to
have moved is the one missing.

## Tier 7 — Property and fuzz

Partial. Property tests exist (`partition_invariants`, `bidirectional`). The
parser is the obvious fuzz candidate and it is tree-sitter's, not ours — a
fuzzer there would test tree-sitter. The first-party candidate is
`UntrustedTrail::from_json`, which by construction parses
attacker-influenced input, and it now has a target (issue #95).

### `trail_from_json`, at `6424bb4`

`fuzz/fuzz_targets/trail_from_json.rs` feeds UTF-8 input to
`UntrustedTrail::from_json` and, when it parses, to `integrity` and
`context_lines` on the same bytes — what a resumed session does with a trail.
`fuzz/` is a cargo-fuzz workspace excluded from the root one, with its own
lock, which G5 audits.

Method: `scripts/checks/fuzz.sh trail_from_json 60` at `6424bb4`, empty
starting corpus, no dictionary. cargo-fuzz 0.13.2, rustc 1.100.0-nightly
(1303417c4 2026-09-21), AddressSanitizer, Windows x86_64, one process.

```
Done 1608601 runs in 61 second(s)
#1608311  REDUCE cov: 1577 ft: 4117 corp: 1744/85Kb ... exec/s: 26805
```

**No crash and no timeout** (LeakSanitizer does not run on Windows). So there is no crashing input to
commit as a regression test in `trellis-trail`.

What this does not establish: one minute on one seed (3204973389) is a
smoke run, not a campaign. It covers panics and memory errors ASan sees; it
says nothing about whether a parsed trail's *content* is handled safely
downstream, which is ADR 0007's fencing and is tested there.

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
| C-025 | 6 | Nine survivors are **unexercisable on this machine**, and the reason differs by cluster. `pull_model`'s four remaining live after the `curl` invocation or would trigger it; `cmd_doctor`'s five sit on `probe_slots`, which needs a live llama.cpp reporting its slot count | **Triaged, not fixed** — with the reason recorded rather than left as an unexplained survivor. The two `pull_model` branches that return *before* curl are tested, because those are the ones that stop a repeat invocation re-downloading a complete file |
| C-024 | 6 | `cmd_why`'s `None if m.task_mentions` guard: replacing it with `true` survives, and is a genuine **equivalent mutant**. `mentions_any` yields a `Mention` only when `touched_file.is_some() \|\| task_mentions`, so a `None` in that arm implies the task mentioned it and the guard cannot be false where it is reached | **Triaged, not fixed** — the guard stays as documentation of the invariant, with a comment at the site saying so. The `false` mutant *is* killed, so the arm itself is tested. Recorded because the next run surfaces it again and an unexplained survivor reads as a test gap |
| C-021 | 6 | `radius_of`'s twelve survivors — the largest single-function cluster in `trellis-cli` — sat on the two branches deciding how much doubt reaches the screen. The `== 0` mutant prints **"every call site in the N files this radius touches was bound"** over a radius with real doubt: a false claim of completeness, which Constraint 1 forbids outright. The existing test greps for `"call site"`, a phrase in *both* branches, so it passed either way | **Fixed** — nine integration tests, each verified against its own mutant. The residue rows are now held to a stated invariant: what is on screen sums to the headline figure |
| C-022 | 6 | `Reason`'s `Display` used `f.write_str`, which ignores the width the caller asked for, so `radius_of`'s `{reason:<22}` was a silent no-op. Every reason row ran ragged while the `(other)` row — a plain `&str`, which does pad — lined up, breaking the column where a reader compares counts | **Fixed** — `f.pad`. Found by writing a test for the residue row and reading the output it printed, not by any mutant: no mutation operator produces "the format spec does nothing" |
| C-023 | — | The TUI could show a typed task and then nothing. A run failing before its first turn emits no tracing event, so the bridge had nothing to forward; `ddc1f59` fixed that and shipped the claim untested, because the event loop refuses a pipe and the path is unreachable from a shell. Separately, `spawn_blocking(..).await` was discarded, so a *panicking* run reported nothing and never handed input back | **Fixed** — `report_outcome` and `lost_run_report` split out and driven through the real `update`; six tests, two of which fail when the forwarding is removed |
| C-001 | 5, 6 | `trellis-cli` had no `tests/`; ten `cmd_*` entry points untested; 829 uncovered lines and 85 of the first 87 missed mutants | **Fixed**, `0ba3b5f` — 20 tests, and it caught `trellis model` refusing to run outside a repository against its own stated intent |
| C-002 | 5, 6 | `trellis_vcs::Worktree::{create,discard}` is unreferenced dead code | **Tested, not deleted** — it has no replacement and v1.1's brief 4.13 specifies it, so dead-and-verified is the right state. Writing the tests found C-010 |
| C-012 | 6 | ADR 0016's rule — the root's own file exempted from clearing `partitionable` — was asserted in neither direction. `replace != with ==` inverts it into the S19 collision the flag prevents | **Fixed** — two tests, one per direction, both verified against the mutant. One also kills `delete ! in blast_radius`, the line carrying `is_partitionable` into the report |
| C-019 | 6 | `Orchestrator::resolve_in_repo` — the guard keeping a model's proposed path inside the tree rollback covers — had no test. The surviving `\|\| -> &&` accepts `../../etc/passwd`, which is relative and so not absolute | **Fixed** — both halves tested, mutant verified dead |
| C-020 | 6 | `Partition::compatible_with` is S19 in one function, and its `&& -> \|\|` survived: two agents handed the same file whenever their call graphs happen to be disjoint. It was only ever tested through `plan`, which cannot produce such a pair | **Fixed** — testing a rule through a caller that cannot violate it proves the caller, not the rule |
| C-018 | 6 | `trellis-checks`, `trellis-core` and `trellis-model` survivors: the two mutants deciding Verified from Reviewed, a crate whose one test could not reach its own code, and the GiB figure users check their disk against | **Fixed** — 6 tests, all verified dead. `trellis-checks` and `trellis-model` are now at zero survivors |
| C-017 | 6 | `trellis-trail`'s remaining survivors: the calendar behind every trail filename was correct at three dates and unasserted at the rest, `run_id`'s filename fallback was untested, and the integrity verdict users see had no assertion | **Fixed** — 4 tests. Five calendar mutants verified dead, and `mp - 9` vs `mp / 9` is separated by a February date alone |
| C-016 | 6 | Trail tamper-evidence worked only because `schemars` transitively enables `serde_json/preserve_order`. Declared nowhere, tested by nothing, and dropping that dependency would make every trail read as `Altered` | **Fixed** — both sides now hash through `Value`, so they agree whatever `Map` is. Verified in a build with the feature and one without |
| C-015 | 6 | `type_head` — what an annotation says the receiver is — had six survivors. The suite tested its effects through fixtures whose annotations are all a bare class name, so unions, quoted forward references, the subscript rule and the character class were unexercised | **Fixed** — five tests, four verified against their mutants |
| C-014 | 6 | `discover` — what the graph counts as first-party — was reached by no test. The suite builds every index from in-memory sources, so the skip-list, the extension filter and the sort that makes ids stable were all unexercised | **Fixed** — five tests, three verified against their mutants |
| C-013 | 6 | Dead code, fourth instance: `RadiusReport::all_edges_certain` and `radius::note_unresolved`, five survivors between them, zero references | **Fixed** — deleted |
| C-011 | 6 | Four guards in the resolver's attribute path could be replaced with `true` and the suite stayed green. Each mutant **emits an unsound edge** — a `Resolved` naming one of several candidates, or an attribute resolved into a function's body | **Fixed** — four tests, each verified against its own mutant. v1.0 gate condition 2 is "zero unsound edges", and the suite did not defend it on this path |
| C-010 | 6 | `Worktree::create` puts a git worktree at `.trellis/work/<run>/partition-<n>`, inside the repository. A linked worktree there reads as untracked, so a run that partitions dirties the tree and **the next run refuses to start** (ADR 0009) | **Fixed** — `work/` added to the `.trellis/.gitignore` the index cache scaffolds. The same wall `index.bin` built once already, in a different file |
| C-008 | 6 | `Trail::factual_is_intact` was dead code, and its three surviving mutants said nothing about the tests | **Fixed** — deleted. The integrity check actually wired in is `UntrustedTrail::integrity`, which takes the raw JSON off disk rather than a parsed `Trail`, because a trail is read back as untrusted data (S10) |
| C-009 | 6 | Per-crate mutation runs test only that crate, so dependents' tests never run and every score understates | **Measured** — `Interner::deserialize -> Default` survives `-p trellis-core` (1 test) and is killed twice by the workspace suite. `--test-workspace true` re-runs in progress for the four affordable crates |
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
