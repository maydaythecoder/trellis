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
| Tests | **1,625** passing, 0 failed, at `989b2a2`, under `cargo llvm-cov --workspace --summary-only` (`docs/bench/coverage-20261002.md`). 487 as recorded in `a23d32f`, 328 at the baseline commit, 415 when the campaign's tiers closed |
| Line coverage | **91.05%** (regions 91.09%, functions 91.35%), at `989b2a2`, `cargo llvm-cov --workspace --summary-only`; see "Tiers 5 and 6 — re-measured" and `docs/bench/coverage-20261002.md`. 82.74% at `94c285c` before it |
| Mutation score | Self-test scores (`-p` only). `trellis-cli`, `trellis-model`, `trellis-vault` and `trellis-queue` re-measured at `989b2a2`: **unusable, no score**, each with 11-20 timeouts, nearly all mutants that make a loop never end; counts and all 847 survivors' dispositions in "Tiers 5 and 6 — re-measured". The other six crates' figures are older; see Tier 6 — final |

The coverage figure and the four re-measured crates' mutation counts are
against `989b2a2`; the other crates' mutation figures are against the commits
in Tier 6. A score quoted without its commit is not a baseline, because
the next commit moves it — the test count above included, because it
changes every commit.

**The 41.5% trellis-cli figure from `4cefe4a` is superseded.** It was
called stale in a known direction here, with 97 of its 144 survivors since
accounted for. The re-run at `989b2a2` replaces it with counts and no score:
1,089 caught and 407 missed of 1,667 mutants, plus 18 timeouts that the
script's own rule says make a percentage meaningless.

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

## Tiers 5 and 6 — re-measured at `989b2a2` (#17)

Measured 2026-10-02/03 on branch `drain`, Windows 11, 16 logical cores,
15.2 GB RAM, the pinned toolchain, with no model runtime running.

### Coverage

`cargo llvm-cov --workspace --summary-only` at `989b2a2`: **lines 91.05%**,
regions 91.09%, functions 91.35%; 1,625 tests passed under instrumentation, 0
failed. The table and the method are in `docs/bench/coverage-20261002.md`. The
earlier 82.74% was measured at `94c285c` on about 30k fewer lines, so the two
are figures for different code, not a before and after.

### Mutation: four crates, self-test, all four unusable as a score

`scripts/mutate-by-crate.sh trellis-model trellis-vault trellis-queue
trellis-cli` at `989b2a2`, after deleting the old trellis-cli and trellis-model
results as the script requires. `-p` only: each crate's own tests, `-j 4`. The
raw lists and `COMMIT` are under `docs/bench/mutants-by-crate/<crate>/`.

```
crate           caught  missed  timeout  unviable  wall-clock
trellis-model      390      83       11        40      26 min
trellis-vault      661     172       20        38      19 min
trellis-queue      897     185       18        83      38 min
trellis-cli       1089     407       18       153    10 h
```

**No score is given for any of the four.** Each printed the script's
`WARNING ... timeouts — treat this result as unusable` line (more than two
timeouts), and a timeout is neither caught nor missed. The script's closing
summary table prints a percentage for every crate anyway. Those percentages are
the arithmetic the warning exists to forbid, and are not quoted here.

**A re-run would not change this,** so the rule's "re-run" branch was not
taken. The timeouts are not machine contention, the usual cause and the one the
rule was written for. Unviable rates of 4-10% are inside the healthy band, and
memory stayed at 5-9 GB free throughout. What timed out is, in most cases, a
mutant that makes a loop or a wait never finish:

- trellis-model: `attempt < MAX_ATTEMPTS` → `true`, `+=` → `*=` on a counter
  that starts at 0, `pull`'s `<` → `==`, `chunked`'s `+=` → `-=`.
- trellis-vault: `next_link` and `match_link` returning a constant `Some(..)`,
  so the scan never advances (8 of the 20); `+=` → `*=` from 0.
- trellis-queue: `Cron::next_after` returning a constant; retry guards
  forced `true`; constant timestamps where a fresh name is generated.
- trellis-cli: `Running::try_wait` → `Ok(None)`; `in_flight` and `abort_run`
  guards inverted; `SteeringPort::close`'s `+=` → `-=`.

Not every timeout was read. Some (`Queue::set_aside` → `Ok(())`, `publish` →
`Ok(false)`, `check_bounds` → `Ok(())`) presumably loop in a caller, and that
was not traced. A per-test time limit would turn these hangs into failures and
make the crates scoreable. Changing that is a decision for its own ticket, not
something to slip into a measurement.

**What changed against the previous measurement,** for the two crates that
had one: trellis-cli's previous run (`4cefe4a`) reported 131 caught and 185
missed out of 333 mutants; this one has 1,667 mutants. trellis-model's
(`8850288`) had 29 caught, 6 missed and **7 timeouts**; this one has 83
survivors. Neither crate is comparable across the two runs, because the mutant
set grew with the code. The earlier trellis-model result also had more than two
timeouts, so by the same rule it was never usable as a score, although Tier 6
— final above quotes it as 82.9%.

trellis-cli after the main.rs split (#126): caught 1,093, missed 408, timeout 13,
unviable 153 of 1,667, at `2024c36`, by `scripts/mutate-by-crate.sh trellis-cli`
(crate-local tests, `-p trellis-cli`, `-j 4`, 11 h). The #17 baseline on the
unsplit file: caught 1,089, missed 407, timeout 18, unviable 153 of 1,667, at
`989b2a2`, same method. Both runs exceed the script's two-timeout limit, so
neither has a caught fraction and none is compared here. The comparison is
made mutant by mutant instead, matching on function and mutation with
`file:line:col` ignored, because the split moved code between files. **No
mutant the baseline caught survives after the split.** One mutant is missed now
and was not missed before, `replace == with != in services_line`
(`doctor_extra.rs`, not a moved file); in the baseline it was a timeout, not a
catch. Five baseline timeouts resolved, four to caught and that one to missed.
No survivor is in a moved function that the baseline killed, so the
"each survivor in a moved function" triage does not apply, and no test was
added.

### Survivors and their dispositions

The ticket asks for every survivor of trellis-vault and trellis-queue, and every
survivor of trellis-cli and trellis-model **not in the crate's previous
`missed.txt`**. cargo-mutants names a mutant by `file:line:col`, and after the
code grew no trellis-cli or trellis-model line matches its old list
character for character. So all 83 + 172 + 185 + 407 = **847** are listed.
With `line:col` ignored, 47 of trellis-cli's 407 match a previous survivor at a
moved position. None of trellis-model's 83 do.

Dispositions: one survivor is unexercisable by `cargo test` and says why. Every
other one is `#<issue>` filed for it, one issue per crate (#245 trellis-model,
#246 trellis-vault, #247 trellis-queue, #248 trellis-cli). Each issue lists its
mutants by file and asks first for a `--test-workspace true` run, because a
self-test survivor that another crate's tests kill is not a gap. No survivor
was killed by a test in this ticket and none was judged equivalent. Calling a
mutant equivalent is a claim about all inputs, and 847 of those were not
going to be made carefully in one sitting.

#245 then gave each trellis-model line its disposition: 75 killed by the
named test in `trellis-model` itself (each shown by applying the mutant by
hand and watching that test fail), 6 equivalent and 2 unexercisable, each
with its reason. None had moved out of existence since `989b2a2`. cargo-mutants
was not re-run for it.

<details>
<summary>trellis-model: 83 survivors</summary>

```
unexercisable by `cargo test`: called only from `fuzz/fuzz_targets/`, which `cargo fuzz` runs on nightly, not by the suite cargo-mutants drives | crates/trellis-model/src/lib.rs:51:5: replace fuzz_parse_response with ()
killed (#245) by hosted::tests::config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else | crates/trellis-model/src/hosted.rs:42:9: replace Protocol::parse_config -> Option<Self> with None
killed (#245) by hosted::tests::config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else | crates/trellis-model/src/hosted.rs:43:13: delete match arm "openai" | "openai-chat" in Protocol::parse_config
killed (#245) by hosted::tests::each_protocol_names_its_own_key_variable_and_its_api | crates/trellis-model/src/hosted.rs:51:9: replace Protocol::default_key_env -> &'static str with ""
killed (#245) by hosted::tests::config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else | crates/trellis-model/src/hosted.rs:44:13: delete match arm "anthropic" in Protocol::parse_config
killed (#245) by hosted::tests::each_protocol_names_its_own_key_variable_and_its_api | crates/trellis-model/src/hosted.rs:51:9: replace Protocol::default_key_env -> &'static str with "xyzzy"
killed (#245) by hosted::tests::each_protocol_names_its_own_key_variable_and_its_api | crates/trellis-model/src/hosted.rs:59:9: replace Protocol::describe -> &'static str with ""
killed (#245) by hosted::tests::each_protocol_names_its_own_key_variable_and_its_api | crates/trellis-model/src/hosted.rs:59:9: replace Protocol::describe -> &'static str with "xyzzy"
killed (#245) by hosted::tests::config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else | crates/trellis-model/src/hosted.rs:78:9: replace Thinking::parse -> Option<Self> with None
killed (#245) by hosted::tests::config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else | crates/trellis-model/src/hosted.rs:78:9: replace Thinking::parse -> Option<Self> with Some(Default::default())
killed (#245) by hosted::tests::config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else | crates/trellis-model/src/hosted.rs:79:13: delete match arm "off" in Thinking::parse
killed (#245) by hosted::tests::config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else | crates/trellis-model/src/hosted.rs:80:13: delete match arm "low" in Thinking::parse
killed (#245) by hosted::tests::config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else | crates/trellis-model/src/hosted.rs:81:13: delete match arm "medium" in Thinking::parse
killed (#245) by hosted::tests::config_words_parse_to_their_protocol_and_thinking_level_and_nothing_else | crates/trellis-model/src/hosted.rs:82:13: delete match arm "high" in Thinking::parse
killed (#245) by hosted::tests::a_reply_is_read_into_one_call_or_a_reason_there_is_not_one | crates/trellis-model/src/hosted.rs:422:25: delete match arm Some("max_tokens") in parse_reply
killed (#245) by hosted::tests::a_reply_is_read_into_one_call_or_a_reason_there_is_not_one | crates/trellis-model/src/hosted.rs:425:25: delete match arm Some("refusal") in parse_reply
killed (#245) by hosted::tests::a_status_not_worth_retrying_is_reported_at_once_with_the_key_scrubbed | crates/trellis-model/src/hosted.rs:488:5: replace retryable -> bool with true
killed (#245) by hosted::tests::retry_after_is_honoured_and_capped | crates/trellis-model/src/hosted.rs:517:33: replace && with || in retry_delay_at
killed (#245) by hosted::tests::a_malformed_or_obsolete_http_date_falls_back_to_the_backoff | crates/trellis-model/src/hosted.rs:561:9: replace || with && in imf_fixdate_unix_secs
killed (#245) by hosted::tests::a_malformed_or_obsolete_http_date_falls_back_to_the_backoff | crates/trellis-model/src/hosted.rs:560:9: replace || with && in imf_fixdate_unix_secs
killed (#245) by hosted::tests::a_malformed_or_obsolete_http_date_falls_back_to_the_backoff | crates/trellis-model/src/hosted.rs:558:9: replace || with && in imf_fixdate_unix_secs
killed (#245) by hosted::tests::a_malformed_or_obsolete_http_date_falls_back_to_the_backoff | crates/trellis-model/src/hosted.rs:559:9: replace || with && in imf_fixdate_unix_secs
killed (#245) by hosted::tests::a_malformed_or_obsolete_http_date_falls_back_to_the_backoff | crates/trellis-model/src/hosted.rs:557:9: replace || with && in imf_fixdate_unix_secs
killed (#245) by hosted::tests::a_malformed_or_obsolete_http_date_falls_back_to_the_backoff | crates/trellis-model/src/hosted.rs:576:39: replace % with / in imf_fixdate_unix_secs
killed (#245) by hosted::tests::a_malformed_or_obsolete_http_date_falls_back_to_the_backoff | crates/trellis-model/src/hosted.rs:576:39: replace % with + in imf_fixdate_unix_secs
killed (#245) by hosted::tests::an_http_date_in_the_future_waits_the_difference | crates/trellis-model/src/hosted.rs:576:64: replace == with != in imf_fixdate_unix_secs
killed (#245) by hosted::tests::an_http_date_in_the_future_waits_the_difference | crates/trellis-model/src/hosted.rs:576:58: replace % with + in imf_fixdate_unix_secs
killed (#245) by hosted::tests::an_http_date_in_the_future_waits_the_difference | crates/trellis-model/src/hosted.rs:576:58: replace % with / in imf_fixdate_unix_secs
killed (#245) by hosted::tests::a_malformed_or_obsolete_http_date_falls_back_to_the_backoff | crates/trellis-model/src/hosted.rs:580:9: delete match arm 4 | 6 | 9 | 11 in imf_fixdate_unix_secs
killed (#245) by hosted::tests::an_http_date_in_the_future_waits_the_difference | crates/trellis-model/src/hosted.rs:584:78: replace > with >= in imf_fixdate_unix_secs
killed (#245) by hosted::tests::a_client_reports_what_it_was_built_with_and_builds_its_body_from_it | crates/trellis-model/src/hosted.rs:710:9: replace HostedClient::base -> &str with ""
killed (#245) by hosted::tests::a_client_reports_what_it_was_built_with_and_builds_its_body_from_it | crates/trellis-model/src/hosted.rs:710:9: replace HostedClient::base -> &str with "xyzzy"
killed (#245) by hosted::tests::a_client_reports_what_it_was_built_with_and_builds_its_body_from_it | crates/trellis-model/src/hosted.rs:714:9: replace HostedClient::model -> &str with ""
killed (#245) by hosted::tests::a_client_reports_what_it_was_built_with_and_builds_its_body_from_it | crates/trellis-model/src/hosted.rs:714:9: replace HostedClient::model -> &str with "xyzzy"
killed (#245) by hosted::tests::a_client_reports_what_it_was_built_with_and_builds_its_body_from_it | crates/trellis-model/src/hosted.rs:718:9: replace HostedClient::thinking -> Thinking with Default::default()
killed (#245) by hosted::tests::a_client_reports_what_it_was_built_with_and_builds_its_body_from_it | crates/trellis-model/src/hosted.rs:723:9: replace HostedClient::key_source -> Option<&str> with None
killed (#245) by hosted::tests::a_client_reports_what_it_was_built_with_and_builds_its_body_from_it | crates/trellis-model/src/hosted.rs:723:9: replace HostedClient::key_source -> Option<&str> with Some("")
killed (#245) by hosted::tests::a_client_reports_what_it_was_built_with_and_builds_its_body_from_it | crates/trellis-model/src/hosted.rs:723:9: replace HostedClient::key_source -> Option<&str> with Some("xyzzy")
killed (#245) by hosted::tests::a_client_reports_what_it_was_built_with_and_builds_its_body_from_it | crates/trellis-model/src/hosted.rs:728:9: replace HostedClient::body -> Value with Default::default()
killed (#245) by hosted::tests::a_request_broken_mid_flight_is_retried_up_to_the_cap_and_no_further | crates/trellis-model/src/hosted.rs:821:46: replace match guard attempt < MAX_ATTEMPTS with true in HostedClient::send_via
killed (#245) by hosted::tests::a_request_broken_mid_flight_is_retried_up_to_the_cap_and_no_further | crates/trellis-model/src/hosted.rs:821:46: replace match guard attempt < MAX_ATTEMPTS with false in HostedClient::send_via
killed (#245) by hosted::tests::a_request_broken_mid_flight_is_retried_up_to_the_cap_and_no_further | crates/trellis-model/src/hosted.rs:821:54: replace < with == in HostedClient::send_via
killed (#245) by hosted::tests::a_request_broken_mid_flight_is_retried_up_to_the_cap_and_no_further | crates/trellis-model/src/hosted.rs:821:54: replace < with > in HostedClient::send_via
killed (#245) by hosted::tests::a_request_broken_mid_flight_is_retried_up_to_the_cap_and_no_further | crates/trellis-model/src/hosted.rs:821:54: replace < with <= in HostedClient::send_via
killed (#245) by hosted::tests::a_status_not_worth_retrying_is_reported_at_once_with_the_key_scrubbed | crates/trellis-model/src/hosted.rs:870:9: replace HostedClient::scrub -> String with String::new()
killed (#245) by hosted::tests::a_status_not_worth_retrying_is_reported_at_once_with_the_key_scrubbed | crates/trellis-model/src/hosted.rs:870:9: replace HostedClient::scrub -> String with "xyzzy".into()
killed (#245) by image::tests::the_bound_is_5_mib_as_anthropic_states_it | crates/trellis-model/src/image.rs:16:45: replace * with +
killed (#245) by image::tests::the_bound_is_5_mib_as_anthropic_states_it | crates/trellis-model/src/image.rs:16:38: replace * with +
equivalent (#245): `Debug` text only; the mutant prints less, never the image data, and no code reads the text | crates/trellis-model/src/image.rs:31:9: replace <impl std::fmt::Debug for Image>::fmt -> std::fmt::Result with Ok(Default::default())
killed (#245) by image::tests::an_image_carries_its_encoding_and_counts_its_length | crates/trellis-model/src/image.rs:75:9: replace Image::base64_len -> usize with 0
killed (#245) by image::tests::an_image_carries_its_encoding_and_counts_its_length | crates/trellis-model/src/image.rs:75:9: replace Image::base64_len -> usize with 1
killed (#245) by image::tests::an_image_carries_its_encoding_and_counts_its_length | crates/trellis-model/src/image.rs:79:9: replace Image::base64 -> &str with "xyzzy"
equivalent (#245): each byte is shifted into its own 8 bits of a word that starts at 0, so `|` and `^` combine disjoint bits identically | crates/trellis-model/src/image.rs:106:40: replace | with ^ in base64
killed (#245) by pull::tests::the_staging_name_is_the_file_name_plus_part | crates/trellis-model/src/pull.rs:74:5: replace partial_name -> String with "xyzzy".into()
killed (#245) by pull::tests::a_file_of_the_pinned_length_at_the_final_name_is_not_fetched_or_hashed | crates/trellis-model/src/pull.rs:81:5: replace already_here -> Result<Option<PathBuf>, PullError> with Ok(None)
killed (#245) by pull::tests::a_short_download_is_kept_to_resume_and_not_moved_into_place | crates/trellis-model/src/pull.rs:120:24: delete ! in pull
killed (#245) by pull::tests::a_final_path_that_cannot_be_inspected_is_an_error_not_absent | crates/trellis-model/src/pull.rs:175:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in len
killed (#245) by runtime::tests::a_runtime_reply_exactly_at_the_cap_is_read | crates/trellis-model/src/runtime.rs:405:22: replace > with >= in RuntimeClient::request
killed (#245) by runtime::tests::a_props_body_that_is_not_json_says_what_it_was_reading | crates/trellis-model/src/runtime.rs:428:5: replace head -> String with String::new()
killed (#245) by runtime::tests::a_props_body_that_is_not_json_says_what_it_was_reading | crates/trellis-model/src/runtime.rs:428:5: replace head -> String with "xyzzy".into()
killed (#245) by secret::tests::from_env_reads_a_set_variable_and_names_it_as_the_source | crates/trellis-model/src/secret.rs:26:9: replace ApiKey::from_env -> Option<Self> with None
killed (#245) by secret::tests::scrub_leaves_text_alone_for_a_key_under_four_characters | crates/trellis-model/src/secret.rs:60:29: replace < with == in ApiKey::scrub
killed (#245) by secret::tests::scrub_leaves_text_alone_for_a_key_under_four_characters | crates/trellis-model/src/secret.rs:60:29: replace < with <= in ApiKey::scrub
killed (#245) by transport::tests::a_connect_answer_with_a_few_kib_of_headers_opens_the_tunnel | crates/trellis-model/src/transport.rs:313:36: replace * with +
killed (#245) by transport::tests::post_sends_https_through_a_proxy_and_never_plain_http | crates/trellis-model/src/transport.rs:433:24: replace match guard ep.tls with true in post
killed (#245) by transport::tests::post_sends_https_through_a_proxy_and_never_plain_http | crates/trellis-model/src/transport.rs:433:24: replace match guard ep.tls with false in post
unexercisable (#245): `#[cfg(not(feature = "hosted"))]`, so not compiled in the default build cargo-mutants tests; without the feature the mutant does not compile (`Response` has no `Default`) | crates/trellis-model/src/transport.rs:471:5: replace tls_exchange -> Result<Response, SendError> with Ok(Default::default())
killed (#245) by transport::tests::a_close_without_close_notify_ends_the_read_and_no_other_error_does | crates/trellis-model/src/transport.rs:533:23: replace match guard e.kind() == std::io::ErrorKind::UnexpectedEof with true in exchange
killed (#245) by transport::tests::a_close_without_close_notify_ends_the_read_and_no_other_error_does | crates/trellis-model/src/transport.rs:533:23: replace match guard e.kind() == std::io::ErrorKind::UnexpectedEof with false in exchange
killed (#245) by transport::tests::a_response_exactly_at_the_cap_is_read_whole | crates/trellis-model/src/transport.rs:518:34: replace > with >= in exchange
killed (#245) by transport::tests::a_response_exactly_at_the_cap_is_read_whole | crates/trellis-model/src/transport.rs:518:30: replace + with * in exchange
killed (#245) by transport::tests::a_response_exactly_at_the_cap_is_read_whole | crates/trellis-model/src/transport.rs:527:56: replace > with >= in exchange
killed (#245) by transport::tests::a_close_without_close_notify_ends_the_read_and_no_other_error_does | crates/trellis-model/src/transport.rs:533:32: replace == with != in exchange
equivalent (#245): moves only `examined`, a counter no production code reads; the linearity test still counts a real framer regression | crates/trellis-model/src/transport.rs:595:27: replace += with *= in Framer::complete
equivalent (#245): moves only `examined`, a counter no production code reads; the linearity test still counts a real framer regression | crates/trellis-model/src/transport.rs:595:40: replace - with + in Framer::complete
killed (#245) by transport::tests::a_chunk_size_line_is_bounded_at_max_size_line_bytes | crates/trellis-model/src/transport.rs:707:67: replace + with - in chunked
killed (#245) by transport::tests::a_chunk_size_line_is_bounded_at_max_size_line_bytes | crates/trellis-model/src/transport.rs:707:67: replace + with * in chunked
killed (#245) by transport::tests::a_chunk_size_line_is_bounded_at_max_size_line_bytes | crates/trellis-model/src/transport.rs:710:39: replace > with == in chunked
killed (#245) by transport::tests::a_chunk_size_line_is_bounded_at_max_size_line_bytes | crates/trellis-model/src/transport.rs:710:39: replace > with >= in chunked
equivalent (#245): at `rest.len() == n` the mutant parks in `Data(0)` at the same `pos`, and the next pass reaches `DataEnd` there, as the original does at once; both return false | crates/trellis-model/src/transport.rs:726:31: replace < with <= in chunked
killed (#245) by transport::tests::bytes_after_a_chunk_that_are_not_crlf_end_the_read_at_once | crates/trellis-model/src/transport.rs:735:31: replace < with <= in chunked
equivalent (#245): moves only `examined`, a counter no production code reads; it cannot underflow, as every size line adds at least 3 first | crates/trellis-model/src/transport.rs:738:27: replace += with -= in chunked
killed (#245) by transport::tests::a_chunked_body_whose_data_all_arrived_is_missing_its_next_size_line | crates/trellis-model/src/transport.rs:802:20: replace < with <= in dechunk
```

</details>

<details>
<summary>trellis-vault: 172 survivors (first measurement)</summary>

Dispositioned by #246. Each mutant was applied by hand to the code at the
ticket's base (`5ac5bf0`), `cargo test -p trellis-vault` was run, and the
source restored; neither cargo-mutants nor the workspace suite was run. 158
are killed by a test #246 added, named on the line (two of those tests never
finish under their mutant, which cargo-mutants reports as a timeout). Four
were already killed at the base: three by a test added after `989b2a2`, and
one by an older test because the code it covers changed. Nine are
equivalent and one cannot be exercised by a deterministic test, each with
its reason. None was left unexamined, and none showed a bug. Since
`989b2a2` retitle's two searches became one `find`, so its three mutants
map onto one closure and two of them are the same mutation at the base.

```
equivalent: the one extra pass sees `&body[len..]`, the empty slice, which `match_link` refuses, and steps past the end | crates/trellis-vault/src/compile.rs:43:13: replace < with <= in link_stems
killed by a_match_resumes_after_its_closing_brackets (#246) | crates/trellis-vault/src/compile.rs:79:23: replace - with / in match_link
killed by a_match_resumes_after_its_closing_brackets (#246) | crates/trellis-vault/src/compile.rs:79:36: replace + with - in match_link
killed by the_byte_limit_on_raw_input_is_16_mib_inclusive, the_line_limits_are_inclusive_per_note_and_in_total and more (#246) | crates/trellis-vault/src/dates.rs:24:38: replace * with +
killed by the_byte_limit_on_raw_input_is_16_mib_inclusive, the_line_limits_are_inclusive_per_note_and_in_total and more (#246) | crates/trellis-vault/src/dates.rs:24:31: replace * with +
killed by the_line_limit_counts_lines_not_newlines (#246) | crates/trellis-vault/src/dates.rs:134:39: replace > with >= in logical_lines
killed by the_line_limit_counts_lines_not_newlines (#246) | crates/trellis-vault/src/dates.rs:134:35: replace + with * in logical_lines
killed by normalize_day_refuses_each_malformed_shape_on_its_own (#246) | crates/trellis-vault/src/dates.rs:156:9: replace || with && in normalize_day
killed by normalize_day_refuses_each_malformed_shape_on_its_own (#246) | crates/trellis-vault/src/dates.rs:155:9: replace || with && in normalize_day
killed by normalize_day_refuses_each_malformed_shape_on_its_own (#246) | crates/trellis-vault/src/dates.rs:154:9: replace || with && in normalize_day
killed by normalize_day_refuses_each_malformed_shape_on_its_own (#246) | crates/trellis-vault/src/dates.rs:153:9: replace || with && in normalize_day
killed by normalize_day_refuses_each_malformed_shape_on_its_own (#246) | crates/trellis-vault/src/dates.rs:152:9: replace || with && in normalize_day
killed by normalize_day_refuses_each_malformed_shape_on_its_own (#246) | crates/trellis-vault/src/dates.rs:167:9: delete match arm 2 in normalize_day
killed by a_heading_is_one_to_six_hashes_then_space_or_end (#246) | crates/trellis-vault/src/dates.rs:230:26: replace && with || in is_heading
killed by a_marker_takes_the_heading_above_it_across_blank_lines (#246) | crates/trellis-vault/src/dates.rs:278:20: replace >= with < in seed
killed by a_marker_takes_the_heading_above_it_across_blank_lines (#246) | crates/trellis-vault/src/dates.rs:279:18: replace -= with += in seed
killed by a_marker_takes_the_heading_above_it_across_blank_lines (#246), which never finishes under it (a timeout to cargo-mutants) | crates/trellis-vault/src/dates.rs:279:18: replace -= with /= in seed
killed by reconcile_keeps_a_common_prefix_of_repeated_lines (#246) | crates/trellis-vault/src/dates.rs:349:18: replace < with == in reconcile
killed by reconcile_keeps_a_common_prefix_of_repeated_lines (#246) | crates/trellis-vault/src/dates.rs:349:18: replace < with > in reconcile
killed by reconcile_keeps_an_interior_line_that_was_first_in_the_old_text (#246) | crates/trellis-vault/src/dates.rs:363:26: replace - with / in reconcile
equivalent: `o == last_old` cannot happen: each old position belongs to one anchor, a kept anchor is used once, and `p - 1` is a prefix line whose anchor then occurs twice in the new text and is not unique | crates/trellis-vault/src/dates.rs:369:27: replace > with >= in reconcile
killed by note_paths_are_refused_for_each_rule_on_its_own (#246) | crates/trellis-vault/src/dates.rs:385:70: replace || with && in valid_note_path
killed by note_paths_are_refused_for_each_rule_on_its_own (#246) | crates/trellis-vault/src/dates.rs:385:47: replace || with && in valid_note_path
killed by note_paths_are_refused_for_each_rule_on_its_own (#246) | crates/trellis-vault/src/dates.rs:385:24: replace || with && in valid_note_path
killed by note_paths_are_refused_for_each_rule_on_its_own (#246) | crates/trellis-vault/src/dates.rs:389:16: replace >= with < in valid_note_path
killed by note_paths_are_refused_for_each_rule_on_its_own (#246) | crates/trellis-vault/src/dates.rs:395:31: replace || with && in valid_note_path
killed by an_anchor_is_32_lowercase_hex_digits (#246) | crates/trellis-vault/src/dates.rs:404:9: replace && with || in valid_anchor
killed by the_note_count_limit_is_inclusive (#246) | crates/trellis-vault/src/dates.rs:414:24: replace > with == in validate
killed by the_note_count_limit_is_inclusive (#246) | crates/trellis-vault/src/dates.rs:414:24: replace > with >= in validate
killed by the_line_limits_are_inclusive_per_note_and_in_total (#246) | crates/trellis-vault/src/dates.rs:427:29: replace > with == in validate
killed by the_line_limits_are_inclusive_per_note_and_in_total (#246) | crates/trellis-vault/src/dates.rs:427:29: replace > with >= in validate
killed by the_line_limits_are_inclusive_per_note_and_in_total (#246) | crates/trellis-vault/src/dates.rs:430:15: replace += with *= in validate
killed by the_line_limits_are_inclusive_per_note_and_in_total (#246) | crates/trellis-vault/src/dates.rs:431:18: replace > with == in validate
killed by the_line_limits_are_inclusive_per_note_and_in_total (#246) | crates/trellis-vault/src/dates.rs:431:18: replace > with >= in validate
killed by the_byte_limit_on_a_document_is_inclusive (#246) | crates/trellis-vault/src/dates.rs:443:35: replace > with == in validate
killed by the_byte_limit_on_a_document_is_inclusive (#246) | crates/trellis-vault/src/dates.rs:443:35: replace > with >= in validate
killed by a_file_at_the_byte_limit_is_read_whole_and_one_over_is_refused_unread, the_byte_limit_on_raw_input_is_16_mib_inclusive (#246) | crates/trellis-vault/src/dates.rs:456:25: replace > with == in parse
killed by a_file_at_the_byte_limit_is_read_whole_and_one_over_is_refused_unread, the_byte_limit_on_raw_input_is_16_mib_inclusive (#246) | crates/trellis-vault/src/dates.rs:456:25: replace > with >= in parse
killed by a_dates_json_symlink_is_refused (#246); the test skips, saying so, where the platform refuses to create a symlink (Windows without developer mode) | crates/trellis-vault/src/dates.rs:469:5: replace is_symlink -> bool with false
killed by a_dates_json_that_is_a_directory_is_invalid_not_absent (#246); exercised on Windows, where opening a directory fails; on Linux the open succeeds and the same test checks the `is_file` refusal instead | crates/trellis-vault/src/dates.rs:484:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in read_bounded
killed by a_file_at_the_byte_limit_is_read_whole_and_one_over_is_refused_unread (#246) | crates/trellis-vault/src/dates.rs:488:24: replace || with && in read_bounded
killed by a_file_at_the_byte_limit_is_read_whole_and_one_over_is_refused_unread (#246) | crates/trellis-vault/src/dates.rs:488:38: replace > with == in read_bounded
killed by a_file_at_the_byte_limit_is_read_whole_and_one_over_is_refused_unread (#246) | crates/trellis-vault/src/dates.rs:488:38: replace > with >= in read_bounded
killed by a_file_at_the_byte_limit_is_read_whole_and_one_over_is_refused_unread (#246) | crates/trellis-vault/src/dates.rs:495:25: replace + with - in read_bounded
unexercisable: by a deterministic test: `take(MAX_BYTES)` instead of `MAX_BYTES + 1` only differs for a file that grows between the size check and the read, which needs a concurrent writer | crates/trellis-vault/src/dates.rs:495:25: replace + with * in read_bounded
killed by a_file_at_the_byte_limit_is_read_whole_and_one_over_is_refused_unread (#246) | crates/trellis-vault/src/dates.rs:496:25: replace > with == in read_bounded
killed by a_file_at_the_byte_limit_is_read_whole_and_one_over_is_refused_unread (#246) | crates/trellis-vault/src/dates.rs:496:25: replace > with >= in read_bounded
killed by a_write_into_a_missing_folder_reports_that_error (#246) | crates/trellis-vault/src/dates.rs:586:23: replace match guard e.kind() == std::io::ErrorKind::AlreadyExists with true in atomic_write
killed by a_taken_temp_name_is_skipped_not_fatal (#246) | crates/trellis-vault/src/dates.rs:586:23: replace match guard e.kind() == std::io::ErrorKind::AlreadyExists with false in atomic_write
killed by a_taken_temp_name_is_skipped_not_fatal, a_write_into_a_missing_folder_reports_that_error (#246) | crates/trellis-vault/src/dates.rs:586:32: replace == with != in atomic_write
caught at the base by coverage_marks_an_over_limit_note_rather_than_reporting_it_empty (added after 989b2a2) | crates/trellis-vault/src/dates.rs:635:5: replace coverage -> (Vec<Coverage>, Vec<String>) with (vec![], vec![])
caught at the base by coverage_marks_an_over_limit_note_rather_than_reporting_it_empty (added after 989b2a2) | crates/trellis-vault/src/dates.rs:635:5: replace coverage -> (Vec<Coverage>, Vec<String>) with (vec![], vec![String::new()])
caught at the base by coverage_marks_an_over_limit_note_rather_than_reporting_it_empty (added after 989b2a2) | crates/trellis-vault/src/dates.rs:635:5: replace coverage -> (Vec<Coverage>, Vec<String>) with (vec![], vec!["xyzzy".into()])
killed by coverage_counts_matching_lines_and_lists_entries_for_no_note (#246) | crates/trellis-vault/src/dates.rs:643:49: replace == with != in coverage
killed by coverage_counts_matching_lines_and_lists_entries_for_no_note (#246) | crates/trellis-vault/src/dates.rs:657:56: replace != with == in coverage
killed by refresh_seeds_from_both_file_days_and_counts_only_changed_notes (#246) | crates/trellis-vault/src/dates.rs:690:21: delete field created_day from struct Seed expression in refresh
killed by refresh_seeds_from_both_file_days_and_counts_only_changed_notes (#246) | crates/trellis-vault/src/dates.rs:691:21: delete field modified_day from struct Seed expression in refresh
killed by refresh_seeds_from_both_file_days_and_counts_only_changed_notes (#246) | crates/trellis-vault/src/dates.rs:699:32: replace += with -= in refresh
killed by refresh_seeds_from_both_file_days_and_counts_only_changed_notes (#246) | crates/trellis-vault/src/dates.rs:698:24: replace != with == in refresh
killed by refresh_seeds_from_both_file_days_and_counts_only_changed_notes (#246) | crates/trellis-vault/src/dates.rs:699:32: replace += with *= in refresh
killed by an_edit_that_touched_nothing_leaves_dates_json_byte_for_byte (#246) | crates/trellis-vault/src/dates.rs:736:9: replace Touched::is_empty -> bool with false
killed by a_created_note_that_already_has_an_entry_is_reconciled_not_reseeded (#246) | crates/trellis-vault/src/dates.rs:766:49: replace && with || in after_edit
killed by a_created_note_that_already_has_an_entry_is_reconciled_not_reseeded (#246) | crates/trellis-vault/src/dates.rs:766:52: delete ! in after_edit
killed by a_layout_json_that_cannot_be_read_fails_the_edit_rather_than_reading_as_absent (#246) | crates/trellis-vault/src/edit.rs:160:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in prune_layout
killed by rm_says_what_it_did_to_the_layout (#246) | crates/trellis-vault/src/edit.rs:192:9: delete match arm Some(Err(e)) in after_set_change
killed by rm_says_what_it_did_to_the_layout (#246) | crates/trellis-vault/src/edit.rs:189:24: replace match guard n > 0 with true in after_set_change
killed by rm_says_what_it_did_to_the_layout (#246) | crates/trellis-vault/src/edit.rs:189:24: replace match guard n > 0 with false in after_set_change
killed by rm_says_what_it_did_to_the_layout (#246) | crates/trellis-vault/src/edit.rs:189:26: replace > with < in after_set_change
killed by rm_says_what_it_did_to_the_layout (#246) | crates/trellis-vault/src/edit.rs:189:26: replace > with == in after_set_change
killed by rm_says_what_it_did_to_the_layout (#246) | crates/trellis-vault/src/edit.rs:189:26: replace > with >= in after_set_change
killed by a_third_note_with_a_taken_title_gets_the_next_suffix (#246) | crates/trellis-vault/src/edit.rs:214:16: replace += with -= in available_title
killed by a_third_note_with_a_taken_title_gets_the_next_suffix (#246), which never finishes under it (a timeout to cargo-mutants) | crates/trellis-vault/src/edit.rs:214:16: replace += with *= in available_title
killed by a_title_with_no_slug_gets_the_untitled_path (#246) | crates/trellis-vault/src/edit.rs:223:14: replace match guard b.is_empty() with false in available_path
killed by retitle_warns_only_when_the_title_is_another_note_s_key (#246); since 989b2a2 retitle's two searches are one `find`, so this and edit.rs:389:27 are the same mutation at HEAD | crates/trellis-vault/src/edit.rs:364:25: replace != with == in retitle
caught at the base by retitle_keeps_a_child_whose_parent_named_the_old_title, which predates 989b2a2: the code changed, not the test; this `&&` is now in the merged `find`, which also decides `collides` | crates/trellis-vault/src/edit.rs:389:32: replace && with || in retitle
killed by retitle_warns_only_when_the_title_is_another_note_s_key (#246); the same mutation as edit.rs:364:25 at HEAD (see there) | crates/trellis-vault/src/edit.rs:389:27: replace != with == in retitle
killed by retitle_counts_the_links_and_parents_it_rewrote (#246) | crates/trellis-vault/src/edit.rs:406:15: replace += with *= in retitle
killed by retitle_counts_the_links_and_parents_it_rewrote (#246) | crates/trellis-vault/src/edit.rs:407:17: replace += with *= in retitle
killed by rm_counts_links_and_orphans_and_is_silent_when_there_are_none (#246) | crates/trellis-vault/src/edit.rs:452:15: replace += with *= in remove
killed by rm_counts_links_and_orphans_and_is_silent_when_there_are_none (#246) | crates/trellis-vault/src/edit.rs:455:18: replace || with && in remove
killed by rm_counts_links_and_orphans_and_is_silent_when_there_are_none (#246) | crates/trellis-vault/src/edit.rs:453:17: replace += with *= in remove
killed by rm_counts_links_and_orphans_and_is_silent_when_there_are_none (#246) | crates/trellis-vault/src/edit.rs:455:14: replace > with == in remove
killed by rm_counts_links_and_orphans_and_is_silent_when_there_are_none (#246) | crates/trellis-vault/src/edit.rs:455:14: replace > with < in remove
killed by rm_counts_links_and_orphans_and_is_silent_when_there_are_none (#246) | crates/trellis-vault/src/edit.rs:455:14: replace > with >= in remove
killed by rm_counts_links_and_orphans_and_is_silent_when_there_are_none (#246) | crates/trellis-vault/src/edit.rs:455:29: replace > with == in remove
killed by rm_counts_links_and_orphans_and_is_silent_when_there_are_none (#246) | crates/trellis-vault/src/edit.rs:455:29: replace > with < in remove
killed by rm_counts_links_and_orphans_and_is_silent_when_there_are_none (#246) | crates/trellis-vault/src/edit.rs:455:29: replace > with >= in remove
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:489:35: replace * with +
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:489:35: replace * with /
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:489:28: replace * with +
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:490:33: replace * with +
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:489:28: replace * with /
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:490:26: replace * with +
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:490:33: replace * with /
killed by the_gate_answer_is_no_for_an_invalid_vault (#246) | crates/trellis-vault/src/edit.rs:508:9: replace Report::gate_ok -> bool with true
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:490:26: replace * with /
killed by fix_lists_the_layout_only_when_it_removed_a_key (#246) | crates/trellis-vault/src/edit.rs:534:14: replace > with == in fix
killed by fix_lists_the_layout_only_when_it_removed_a_key (#246) | crates/trellis-vault/src/edit.rs:534:14: replace > with < in fix
killed by fix_lists_the_layout_only_when_it_removed_a_key (#246) | crates/trellis-vault/src/edit.rs:534:14: replace > with >= in fix
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:562:28: replace >= with < in check
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:568:14: replace >= with < in check
killed by check_does_not_warn_below_apex_s_size_thresholds (#246) | crates/trellis-vault/src/edit.rs:574:20: replace >= with < in check
killed by root_of_follows_the_notes_folder_rule (#246) | crates/trellis-vault/src/edit.rs:598:5: replace root_of -> std::path::PathBuf with Default::default()
killed by a_missing_parent_names_candidates_only_when_it_is_ambiguous, a_shared_title_resolves_as_ambiguous_naming_both (#246) | crates/trellis-vault/src/graph.rs:76:69: replace > with == in Resolver::resolve
killed by a_missing_parent_names_candidates_only_when_it_is_ambiguous, a_shared_title_resolves_as_ambiguous_naming_both (#246) | crates/trellis-vault/src/graph.rs:76:69: replace > with < in Resolver::resolve
equivalent: the first loop has already returned for any one-note bucket, and buckets are never empty, so every bucket reaching this filter has more than one note | crates/trellis-vault/src/graph.rs:76:69: replace > with >= in Resolver::resolve
killed by key_for_is_the_basename_only_when_it_is_unique (#246) | crates/trellis-vault/src/graph.rs:110:38: replace == with != in Resolver::key_for
killed by a_missing_parent_names_candidates_only_when_it_is_ambiguous (#246) | crates/trellis-vault/src/graph.rs:164:20: delete ! in <impl std::fmt::Display for Issue>::fmt
killed by a_missing_parent_names_candidates_only_when_it_is_ambiguous (#246) | crates/trellis-vault/src/graph.rs:236:25: delete match arm Resolved::Ambiguous(c) in Graph::build
killed by lineage_runs_from_the_root_to_the_note (#246) | crates/trellis-vault/src/graph.rs:313:9: replace Graph::lineage -> Vec<usize> with vec![1]
killed by lineage_runs_from_the_root_to_the_note (#246) | crates/trellis-vault/src/graph.rs:313:9: replace Graph::lineage -> Vec<usize> with vec![0]
killed by lineage_runs_from_the_root_to_the_note (#246) | crates/trellis-vault/src/graph.rs:313:9: replace Graph::lineage -> Vec<usize> with vec![]
killed by rm_leaves_the_report_line_counting_a_parent_with_children_as_connected (#246) | crates/trellis-vault/src/graph.rs:350:55: delete ! in Graph::counts
killed by the_graph_reports_a_parent_cycle (#246) | crates/trellis-vault/src/graph.rs:359:5: replace find_cycles -> Vec<Issue> with vec![]
killed by the_first_heading_may_be_on_a_later_line_or_only_whitespace, the_line_terminators_are_javascript_s_four (#246) | crates/trellis-vault/src/js.rs:23:5: replace is_line_terminator -> bool with false
killed by a_file_named_notes_is_not_the_notes_root (#246) | crates/trellis-vault/src/reader.rs:20:18: replace match guard m.is_dir() with true in notes_root
killed by a_key_is_letters_digits_underscore_and_dash_only (#246) | crates/trellis-vault/src/reader.rs:137:9: replace || with && in key_line
killed by a_key_is_letters_digits_underscore_and_dash_only (#246) | crates/trellis-vault/src/reader.rs:139:61: replace || with && in key_line
killed by a_key_is_letters_digits_underscore_and_dash_only (#246) | crates/trellis-vault/src/reader.rs:139:53: replace == with != in key_line
killed by a_key_is_letters_digits_underscore_and_dash_only (#246) | crates/trellis-vault/src/reader.rs:139:66: replace == with != in key_line
killed by unquote_strips_only_a_matching_pair (#246) | crates/trellis-vault/src/reader.rs:155:43: replace && with || in unquote
killed by unquote_strips_only_a_matching_pair (#246) | crates/trellis-vault/src/reader.rs:156:82: replace - with + in unquote
killed by unquote_strips_only_a_matching_pair (#246) | crates/trellis-vault/src/reader.rs:156:82: replace - with / in unquote
killed by unquote_strips_only_a_matching_pair (#246) | crates/trellis-vault/src/reader.rs:158:44: replace && with || in unquote
killed by the_first_heading_may_be_on_a_later_line_or_only_whitespace (#246) | crates/trellis-vault/src/reader.rs:178:27: replace + with - in first_heading
killed by the_first_heading_may_be_on_a_later_line_or_only_whitespace (#246) | crates/trellis-vault/src/reader.rs:178:27: replace + with * in first_heading
killed by the_first_heading_may_be_on_a_later_line_or_only_whitespace (#246) | crates/trellis-vault/src/reader.rs:202:48: replace && with || in first_heading
killed by the_first_heading_may_be_on_a_later_line_or_only_whitespace (#246) | crates/trellis-vault/src/reader.rs:202:13: delete ! in first_heading
killed by the_first_heading_may_be_on_a_later_line_or_only_whitespace (#246) | crates/trellis-vault/src/reader.rs:202:66: replace || with && in first_heading
killed by the_first_heading_may_be_on_a_later_line_or_only_whitespace (#246) | crates/trellis-vault/src/reader.rs:202:54: replace == with != in first_heading
killed by md_is_stripped_after_a_multibyte_character (#246) | crates/trellis-vault/src/refs.rs:60:39: replace - with / in strip_md
killed by graph_links_resume_exactly_after_each_link (#246) | crates/trellis-vault/src/refs.rs:97:20: replace && with || in next_link
killed by graph_links_resume_exactly_after_each_link (#246) | crates/trellis-vault/src/refs.rs:97:16: replace > with >= in next_link
killed by graph_links_resume_exactly_after_each_link (#246) | crates/trellis-vault/src/refs.rs:98:41: replace + with - in next_link
killed by graph_links_resume_exactly_after_each_link (#246) | crates/trellis-vault/src/refs.rs:98:41: replace + with * in next_link
killed by graph_links_resume_exactly_after_each_link (#246) | crates/trellis-vault/src/refs.rs:98:35: replace + with * in next_link
killed by a_target_ending_in_a_slash_is_its_own_label (#246) | crates/trellis-vault/src/refs.rs:122:27: replace match guard !last.is_empty() with true in parse_wiki_target
killed by a_fence_masks_exactly_through_its_closing_backticks (#246) | crates/trellis-vault/src/refs.rs:187:37: replace + with - in mask_fences
killed by a_fence_masks_exactly_through_its_closing_backticks (#246) | crates/trellis-vault/src/refs.rs:187:25: replace + with - in mask_fences
killed by a_rewrite_skips_empty_and_overlong_links_and_counts_only_real_changes (#246) | crates/trellis-vault/src/refs.rs:220:36: replace || with && in rewrite_links
killed by a_rewrite_skips_empty_and_overlong_links_and_counts_only_real_changes (#246) | crates/trellis-vault/src/refs.rs:220:21: replace || with && in rewrite_links
killed by a_rewrite_skips_empty_and_overlong_links_and_counts_only_real_changes (#246) | crates/trellis-vault/src/refs.rs:220:30: replace > with == in rewrite_links
killed by a_rewrite_skips_empty_and_overlong_links_and_counts_only_real_changes (#246) | crates/trellis-vault/src/refs.rs:220:30: replace > with >= in rewrite_links
killed by a_rewrite_skips_empty_and_overlong_links_and_counts_only_real_changes (#246) | crates/trellis-vault/src/refs.rs:237:24: replace match guard r != original with true in rewrite_links
killed by the_body_and_trigram_cutoffs_are_apex_s (#246) | crates/trellis-vault/src/search.rs:21:41: replace * with +
killed by the_body_and_trigram_cutoffs_are_apex_s (#246) | crates/trellis-vault/src/search.rs:21:34: replace * with +
killed by the_body_and_trigram_cutoffs_are_apex_s (#246) | crates/trellis-vault/src/search.rs:23:40: replace * with +
killed by the_body_and_trigram_cutoffs_are_apex_s (#246) | crates/trellis-vault/src/search.rs:23:33: replace * with +
killed by the_body_and_trigram_cutoffs_are_apex_s (#246) | crates/trellis-vault/src/search.rs:45:63: replace && with || in Mode::for_vault
killed by the_body_and_trigram_cutoffs_are_apex_s (#246) | crates/trellis-vault/src/search.rs:45:35: replace && with || in Mode::for_vault
killed by the_body_and_trigram_cutoffs_are_apex_s (#246) | crates/trellis-vault/src/search.rs:48:44: replace && with || in Mode::for_vault
killed by three_characters_make_one_trigram_and_two_make_none (#246) | crates/trellis-vault/src/search.rs:84:16: replace < with == in trigrams
killed by three_characters_make_one_trigram_and_two_make_none (#246) | crates/trellis-vault/src/search.rs:84:16: replace < with <= in trigrams
equivalent: the early return is a shortcut: with no terms the loop adds nothing, and with no documents there are no postings, so both paths return no hits | crates/trellis-vault/src/search.rs:203:29: replace || with && in Index::bm25
equivalent: `average` is 0 only for an empty index, which returned earlier; every document's length is at least 1 | crates/trellis-vault/src/search.rs:215:44: replace > with >= in Index::bm25
killed by a_note_matching_two_terms_scores_their_sum (#246) | crates/trellis-vault/src/search.rs:222:40: replace += with -= in Index::bm25
killed by a_note_matching_two_terms_scores_their_sum (#246) | crates/trellis-vault/src/search.rs:222:40: replace += with *= in Index::bm25
equivalent: the early return is a shortcut: no query trigrams or no documents leave `overlap` empty either way | crates/trellis-vault/src/search.rs:236:25: replace || with && in Index::trigram
equivalent: `union` is at least `q.len()`, which is non-zero past the early return | crates/trellis-vault/src/search.rs:252:34: replace > with >= in Index::trigram
killed by a_full_bm25_page_gets_no_trigram_bonus_and_a_trigram_only_hit_is_weighted (#246) | crates/trellis-vault/src/search.rs:269:40: replace || with && in Index::search
killed by a_full_bm25_page_gets_no_trigram_bonus_and_a_trigram_only_hit_is_weighted (#246) | crates/trellis-vault/src/search.rs:280:21: delete field score from struct Hit expression in Index::search
equivalent: `..t` supplies `bm25`, and a trigram hit's `bm25` is already 0.0 | crates/trellis-vault/src/search.rs:281:21: delete field bm25 from struct Hit expression in Index::search
killed by a_full_bm25_page_gets_no_trigram_bonus_and_a_trigram_only_hit_is_weighted (#246) | crates/trellis-vault/src/search.rs:280:38: replace * with + in Index::search
killed by a_full_bm25_page_gets_no_trigram_bonus_and_a_trigram_only_hit_is_weighted (#246) | crates/trellis-vault/src/search.rs:280:38: replace * with / in Index::search
killed by a_stem_keeps_md_when_it_is_the_whole_name (#246) | crates/trellis-vault/src/validate.rs:97:14: replace match guard !s.is_empty() with true in stem_of
killed by a_parent_link_is_one_non_empty_bracket_free_stem (#246) | crates/trellis-vault/src/validate.rs:119:25: replace || with && in parent_link
equivalent: a title that only ends with `"` reaches `unquote`, whose own check needs the opening quote, and comes back unchanged | crates/trellis-vault/src/validate.rs:185:63: replace && with || in parse_note
killed by the_gate_decodes_only_a_double_quoted_title (#246) | crates/trellis-vault/src/validate.rs:185:37: replace && with || in parse_note
killed by a_manifest_entry_outside_the_vault_is_named_for_each_form (#246) | crates/trellis-vault/src/validate.rs:217:5: replace has_drive -> bool with false
killed by a_manifest_entry_outside_the_vault_is_named_for_each_form (#246) | crates/trellis-vault/src/validate.rs:218:13: replace >= with < in has_drive
killed by a_manifest_entry_outside_the_vault_is_named_for_each_form (#246) | crates/trellis-vault/src/validate.rs:409:47: replace || with && in manifest_state
killed by a_manifest_entry_outside_the_vault_is_named_for_each_form (#246) | crates/trellis-vault/src/validate.rs:409:31: replace || with && in manifest_state
```

</details>

<details>
<summary>trellis-queue: 185 survivors (first measurement)</summary>

```
#247 | crates/trellis-queue/src/lib.rs:141:9: replace Status::as_str -> &'static str with ""
#247 | crates/trellis-queue/src/lib.rs:141:9: replace Status::as_str -> &'static str with "xyzzy"
#247 | crates/trellis-queue/src/lib.rs:154:9: replace Status::parse -> Option<Status> with None
#247 | crates/trellis-queue/src/lib.rs:155:13: delete match arm "backlog" in Status::parse
#247 | crates/trellis-queue/src/lib.rs:156:13: delete match arm "todo" in Status::parse
#247 | crates/trellis-queue/src/lib.rs:157:13: delete match arm "in_progress" in Status::parse
#247 | crates/trellis-queue/src/lib.rs:158:13: delete match arm "in_review" in Status::parse
#247 | crates/trellis-queue/src/lib.rs:159:13: delete match arm "blocked" in Status::parse
#247 | crates/trellis-queue/src/lib.rs:160:13: delete match arm "done" in Status::parse
#247 | crates/trellis-queue/src/lib.rs:161:13: delete match arm "cancelled" in Status::parse
#247 | crates/trellis-queue/src/lib.rs:162:13: delete match arm "failed" in Status::parse
#247 | crates/trellis-queue/src/lib.rs:223:28: replace < with == in Blocked::describe
#247 | crates/trellis-queue/src/lib.rs:223:28: replace < with > in Blocked::describe
#247 | crates/trellis-queue/src/lib.rs:258:9: replace Priority::parse -> Option<Priority> with None
#247 | crates/trellis-queue/src/lib.rs:258:9: replace Priority::parse -> Option<Priority> with Some(Default::default())
#247 | crates/trellis-queue/src/lib.rs:249:9: replace Priority::as_str -> &'static str with ""
#247 | crates/trellis-queue/src/lib.rs:249:9: replace Priority::as_str -> &'static str with "xyzzy"
#247 | crates/trellis-queue/src/lib.rs:223:28: replace < with <= in Blocked::describe
#247 | crates/trellis-queue/src/lib.rs:259:13: delete match arm "critical" in Priority::parse
#247 | crates/trellis-queue/src/lib.rs:262:13: delete match arm "low" in Priority::parse
#247 | crates/trellis-queue/src/lib.rs:261:13: delete match arm "medium" in Priority::parse
#247 | crates/trellis-queue/src/lib.rs:260:13: delete match arm "high" in Priority::parse
#247 | crates/trellis-queue/src/lib.rs:397:25: replace match guard st.home() == s with true in Task::status_in
#247 | crates/trellis-queue/src/lib.rs:442:49: replace + with * in Task::answers_since_question
#247 | crates/trellis-queue/src/lib.rs:655:17: delete field prompt from struct NewTask expression in Queue::add
#247 | crates/trellis-queue/src/lib.rs:680:13: delete field prompt from struct Task expression in Queue::add_task
#247 | crates/trellis-queue/src/lib.rs:681:13: delete field args from struct Task expression in Queue::add_task
#247 | crates/trellis-queue/src/lib.rs:684:13: delete field labels from struct Task expression in Queue::add_task
#247 | crates/trellis-queue/src/lib.rs:801:23: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in Queue::ids
#247 | crates/trellis-queue/src/lib.rs:833:51: replace match guard source.kind() == std::io::ErrorKind::NotFound with true in Queue::claim
#247 | crates/trellis-queue/src/lib.rs:833:51: replace match guard source.kind() == std::io::ErrorKind::NotFound with false in Queue::claim
#247 | crates/trellis-queue/src/lib.rs:841:20: replace match guard source.kind() == std::io::ErrorKind::PermissionDenied && (!from.exists() || to.exists()) with true in Queue::claim
#247 | crates/trellis-queue/src/lib.rs:841:20: replace match guard source.kind() == std::io::ErrorKind::PermissionDenied && (!from.exists() || to.exists()) with false in Queue::claim
#247 | crates/trellis-queue/src/lib.rs:833:65: replace == with != in Queue::claim
#247 | crates/trellis-queue/src/lib.rs:842:21: replace && with || in Queue::claim
#247 | crates/trellis-queue/src/lib.rs:841:34: replace == with != in Queue::claim
#247 | crates/trellis-queue/src/lib.rs:842:40: replace || with && in Queue::claim
#247 | crates/trellis-queue/src/lib.rs:842:25: delete ! in Queue::claim
#247 | crates/trellis-queue/src/lib.rs:906:23: replace match guard e.kind() == std::io::ErrorKind::AlreadyExists with true in Queue::set_aside
#247 | crates/trellis-queue/src/lib.rs:905:23: replace match guard e.kind() == std::io::ErrorKind::NotFound with false in Queue::set_aside
#247 | crates/trellis-queue/src/lib.rs:921:39: replace match guard e.kind() == std::io::ErrorKind::AlreadyExists with false in Queue::set_aside
#247 | crates/trellis-queue/src/lib.rs:921:39: replace match guard e.kind() == std::io::ErrorKind::AlreadyExists with true in Queue::set_aside
#247 | crates/trellis-queue/src/lib.rs:921:48: replace == with != in Queue::set_aside
#247 | crates/trellis-queue/src/lib.rs:1212:9: replace Queue::fresh_id -> String with String::new()
#247 | crates/trellis-queue/src/lib.rs:1212:9: replace Queue::fresh_id -> String with "xyzzy".into()
#247 | crates/trellis-queue/src/lib.rs:1395:9: replace Runner::now -> i64 with 0
#247 | crates/trellis-queue/src/lib.rs:1383:9: replace Runner::next_due -> Option<Duration> with Some(Default::default())
#247 | crates/trellis-queue/src/lib.rs:1395:9: replace Runner::now -> i64 with 1
#247 | crates/trellis-queue/src/lib.rs:1395:9: replace Runner::now -> i64 with -1
#247 | crates/trellis-queue/src/lib.rs:1401:9: replace Runner::profiles -> Option<&BTreeMap<String, profile::Profile>> with Some(Box::leak(Box::new(BTreeMap::new())))
#247 | crates/trellis-queue/src/lib.rs:1443:9: replace <impl std::fmt::Display for Stop>::fmt -> std::fmt::Result with Ok(Default::default())
#247 | crates/trellis-queue/src/lib.rs:1471:5: replace reconcile_and_say with ()
#247 | crates/trellis-queue/src/lib.rs:1550:12: delete ! in work
#247 | crates/trellis-queue/src/lib.rs:1610:40: replace && with || in work
#247 | crates/trellis-queue/src/activity.rs:34:9: replace Actor::as_str -> &'static str with ""
#247 | crates/trellis-queue/src/activity.rs:34:9: replace Actor::as_str -> &'static str with "xyzzy"
#247 | crates/trellis-queue/src/activity.rs:119:23: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in ActivityLog::tail
#247 | crates/trellis-queue/src/activity.rs:119:23: replace match guard e.kind() == std::io::ErrorKind::NotFound with false in ActivityLog::tail
#247 | crates/trellis-queue/src/activity.rs:119:32: replace == with != in ActivityLog::tail
#247 | crates/trellis-queue/src/approval.rs:43:5: replace is_request_id -> bool with true
#247 | crates/trellis-queue/src/approval.rs:43:5: replace is_request_id -> bool with false
#247 | crates/trellis-queue/src/approval.rs:51:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in list
#247 | crates/trellis-queue/src/approval.rs:59:17: replace && with || in list
#247 | crates/trellis-queue/src/approval.rs:242:13: delete match arm Ok(true) in approve_budget
#247 | crates/trellis-queue/src/cron.rs:192:56: replace || with && in Cron::parse
#247 | crates/trellis-queue/src/cron.rs:194:27: replace > with == in Cron::parse
#247 | crates/trellis-queue/src/cron.rs:194:27: replace > with < in Cron::parse
#247 | crates/trellis-queue/src/cron.rs:194:27: replace > with >= in Cron::parse
#247 | crates/trellis-queue/src/cron.rs:196:40: replace - with + in Cron::parse
#247 | crates/trellis-queue/src/cron.rs:196:40: replace - with / in Cron::parse
#247 | crates/trellis-queue/src/cron.rs:199:43: replace || with && in Cron::parse
#247 | crates/trellis-queue/src/cron.rs:234:9: replace Cron::as_str -> &str with ""
#247 | crates/trellis-queue/src/cron.rs:234:9: replace Cron::as_str -> &str with "xyzzy"
#247 | crates/trellis-queue/src/cron.rs:268:23: replace + with - in Cron::next_after
#247 | crates/trellis-queue/src/cron.rs:269:47: replace + with - in Cron::next_after
#247 | crates/trellis-queue/src/cron.rs:289:44: replace > with >= in Cron::next_after
#247 | crates/trellis-queue/src/cron.rs:319:5: replace tz_source -> String with String::new()
#247 | crates/trellis-queue/src/cron.rs:319:5: replace tz_source -> String with "xyzzy".into()
#247 | crates/trellis-queue/src/cron.rs:334:71: replace || with && in Tz::parse
#247 | crates/trellis-queue/src/cron.rs:334:54: replace || with && in Tz::parse
#247 | crates/trellis-queue/src/cron.rs:362:19: replace || with && in Tz::parse
#247 | crates/trellis-queue/src/cron.rs:362:14: replace > with >= in Tz::parse
#247 | crates/trellis-queue/src/cron.rs:362:14: replace > with == in Tz::parse
#247 | crates/trellis-queue/src/cron.rs:362:24: replace > with == in Tz::parse
#247 | crates/trellis-queue/src/cron.rs:362:24: replace > with >= in Tz::parse
#247 | crates/trellis-queue/src/cron.rs:376:27: replace || with && in Tz::named
#247 | crates/trellis-queue/src/cron.rs:431:34: replace < with <= in <impl std::fmt::Display for Tz>::fmt
#247 | crates/trellis-queue/src/files.rs:50:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in read_settled
#247 | crates/trellis-queue/src/files.rs:83:47: replace match guard source.kind() == std::io::ErrorKind::NotFound with true in read_json_opt
#247 | crates/trellis-queue/src/files.rs:161:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in remove
#247 | crates/trellis-queue/src/files.rs:240:23: replace match guard e.kind() == std::io::ErrorKind::AlreadyExists with false in settle
#247 | crates/trellis-queue/src/files.rs:240:23: replace match guard e.kind() == std::io::ErrorKind::AlreadyExists with true in settle
#247 | crates/trellis-queue/src/files.rs:240:32: replace == with != in settle
#247 | crates/trellis-queue/src/files.rs:259:20: replace match guard e.kind() == std::io::ErrorKind::PermissionDenied && waited < Duration::from_secs(5) with false in patiently
#247 | crates/trellis-queue/src/files.rs:260:31: replace < with == in patiently
#247 | crates/trellis-queue/src/files.rs:260:31: replace < with > in patiently
#247 | crates/trellis-queue/src/files.rs:260:31: replace < with <= in patiently
#247 | crates/trellis-queue/src/files.rs:263:24: replace += with -= in patiently
#247 | crates/trellis-queue/src/files.rs:264:30: replace * with / in patiently
#247 | crates/trellis-queue/src/ledger.rs:65:23: replace += with *= in spend
#247 | crates/trellis-queue/src/ledger.rs:65:23: replace += with -= in spend
#247 | crates/trellis-queue/src/ledger.rs:184:9: delete field paused from struct Overlay expression in enforce
#247 | crates/trellis-queue/src/mention.rs:57:38: replace && with || in mentions
#247 | crates/trellis-queue/src/mention.rs:153:57: replace && with || in Queue::apply
#247 | crates/trellis-queue/src/portable.rs:34:9: replace OnConflict::parse -> Option<Self> with None
#247 | crates/trellis-queue/src/portable.rs:35:13: delete match arm "skip" in OnConflict::parse
#247 | crates/trellis-queue/src/portable.rs:36:13: delete match arm "replace" in OnConflict::parse
#247 | crates/trellis-queue/src/portable.rs:37:13: delete match arm "rename" in OnConflict::parse
#247 | crates/trellis-queue/src/portable.rs:52:9: replace Kind::as_str -> &'static str with ""
#247 | crates/trellis-queue/src/portable.rs:52:9: replace Kind::as_str -> &'static str with "xyzzy"
#247 | crates/trellis-queue/src/portable.rs:110:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in tomls
#247 | crates/trellis-queue/src/portable.rs:160:35: replace += with *= in export
#247 | crates/trellis-queue/src/portable.rs:160:35: replace += with -= in export
#247 | crates/trellis-queue/src/portable.rs:182:9: replace && with || in valid_id
#247 | crates/trellis-queue/src/portable.rs:181:5: replace valid_id -> bool with true
#247 | crates/trellis-queue/src/portable.rs:210:39: replace match guard m.contains("not a profile") with true in plan_import
#247 | crates/trellis-queue/src/portable.rs:234:55: replace || with && in plan_import
#247 | crates/trellis-queue/src/portable.rs:276:51: replace == with != in plan_import
#247 | crates/trellis-queue/src/portable.rs:288:21: replace != with == in plan_import
#247 | crates/trellis-queue/src/portable.rs:310:60: replace || with && in plan_import
#247 | crates/trellis-queue/src/portable.rs:362:46: replace match guard on == OnConflict::Replace with true in plan_import
#247 | crates/trellis-queue/src/portable.rs:439:28: replace == with != in apply_import
#247 | crates/trellis-queue/src/profile.rs:110:9: replace Profile::is_process -> bool with true
#247 | crates/trellis-queue/src/profile.rs:110:9: replace Profile::is_process -> bool with false
#247 | crates/trellis-queue/src/profile.rs:123:9: replace Profile::instructions_path -> Option<PathBuf> with Some(Default::default())
#247 | crates/trellis-queue/src/profile.rs:123:9: replace Profile::instructions_path -> Option<PathBuf> with None
#247 | crates/trellis-queue/src/profile.rs:178:30: replace || with && in validate
#247 | crates/trellis-queue/src/profile.rs:215:62: replace || with && in validate
#247 | crates/trellis-queue/src/profile.rs:215:43: replace || with && in validate
#247 | crates/trellis-queue/src/profile.rs:279:20: replace > with >= in validate
#247 | crates/trellis-queue/src/profile.rs:321:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in load_dir
#247 | crates/trellis-queue/src/routine.rs:90:25: replace + with - in placeholders
#247 | crates/trellis-queue/src/routine.rs:130:23: replace || with && in parse
#247 | crates/trellis-queue/src/routine.rs:130:28: replace > with == in parse
#247 | crates/trellis-queue/src/routine.rs:130:28: replace > with >= in parse
#247 | crates/trellis-queue/src/routine.rs:178:5: replace load_all -> Result<BTreeMap<String, Routine>, QueueError> with Ok(BTreeMap::new())
#247 | crates/trellis-queue/src/routine.rs:182:5: replace load_dir -> Result<BTreeMap<String, Routine>, QueueError> with Ok(BTreeMap::new())
#247 | crates/trellis-queue/src/routine.rs:185:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in load_dir
#247 | crates/trellis-queue/src/routine.rs:269:5: replace state -> Result<Option<RoutineState>, QueueError> with Ok(Some(Default::default()))
#247 | crates/trellis-queue/src/routine.rs:269:5: replace state -> Result<Option<RoutineState>, QueueError> with Ok(None)
#247 | crates/trellis-queue/src/routine.rs:294:5: replace active -> Result<Option<String>, QueueError> with Ok(Some(String::new()))
#247 | crates/trellis-queue/src/routine.rs:294:5: replace active -> Result<Option<String>, QueueError> with Ok(None)
#247 | crates/trellis-queue/src/routine.rs:294:5: replace active -> Result<Option<String>, QueueError> with Ok(Some("xyzzy".into()))
#247 | crates/trellis-queue/src/routine.rs:299:41: replace == with != in active
#247 | crates/trellis-queue/src/routine.rs:344:31: replace match guard claimed.is_none() with true in seen
#247 | crates/trellis-queue/src/routine.rs:344:31: replace match guard claimed.is_none() with false in seen
#247 | crates/trellis-queue/src/routine.rs:345:31: replace match guard pending.is_none() with false in seen
#247 | crates/trellis-queue/src/routine.rs:345:31: replace match guard pending.is_none() with true in seen
#247 | crates/trellis-queue/src/routine.rs:424:9: delete field name from struct Tick expression in evaluate
#247 | crates/trellis-queue/src/routine.rs:473:20: replace > with == in evaluate
#247 | crates/trellis-queue/src/routine.rs:473:20: replace > with < in evaluate
#247 | crates/trellis-queue/src/routine.rs:473:20: replace > with >= in evaluate
#247 | crates/trellis-queue/src/routine.rs:512:5: replace next_due -> Option<i64> with Some(1)
#247 | crates/trellis-queue/src/routine.rs:512:5: replace next_due -> Option<i64> with Some(0)
#247 | crates/trellis-queue/src/routine.rs:512:5: replace next_due -> Option<i64> with None
#247 | crates/trellis-queue/src/routine.rs:512:5: replace next_due -> Option<i64> with Some(-1)
#247 | crates/trellis-queue/src/stale.rs:39:13: delete match arm "list" in StaleClaims::parse
#247 | crates/trellis-queue/src/stale.rs:38:9: replace StaleClaims::parse -> Option<StaleClaims> with None
#247 | crates/trellis-queue/src/stale.rs:38:9: replace StaleClaims::parse -> Option<StaleClaims> with Some(Default::default())
#247 | crates/trellis-queue/src/routine.rs:514:21: delete ! in next_due
#247 | crates/trellis-queue/src/stale.rs:40:13: delete match arm "release" in StaleClaims::parse
#247 | crates/trellis-queue/src/stale.rs:81:9: replace Liveness::os -> String with "xyzzy".into()
#247 | crates/trellis-queue/src/stale.rs:81:9: replace Liveness::os -> String with String::new()
#247 | crates/trellis-queue/src/stale.rs:119:24: replace match guard source.kind() == std::io::ErrorKind::NotFound with true in Queue::release_stale_claims
#247 | crates/trellis-queue/src/stale.rs:119:24: replace match guard source.kind() == std::io::ErrorKind::NotFound with false in Queue::release_stale_claims
#247 | crates/trellis-queue/src/stale.rs:119:38: replace == with != in Queue::release_stale_claims
#247 | crates/trellis-queue/src/stale.rs:215:51: replace match guard source.kind() == std::io::ErrorKind::NotFound with true in Queue::release_dead
#247 | crates/trellis-queue/src/stale.rs:215:51: replace match guard source.kind() == std::io::ErrorKind::NotFound with false in Queue::release_dead
#247 | crates/trellis-queue/src/stale.rs:215:65: replace == with != in Queue::release_dead
#247 | crates/trellis-queue/src/stale.rs:220:52: replace || with && in Queue::release_dead
#247 | crates/trellis-queue/src/stamp.rs:19:48: replace - with / in civil
#247 | crates/trellis-queue/src/stamp.rs:19:33: replace + with - in civil
#247 | crates/trellis-queue/src/stamp.rs:57:5: replace now_secs -> i64 with 0
#247 | crates/trellis-queue/src/stamp.rs:57:5: replace now_secs -> i64 with 1
#247 | crates/trellis-queue/src/stamp.rs:57:5: replace now_secs -> i64 with -1
#247 | crates/trellis-queue/src/stamp.rs:99:71: replace || with && in parse_iso
#247 | crates/trellis-queue/src/stamp.rs:99:54: replace || with && in parse_iso
#247 | crates/trellis-queue/src/stamp.rs:99:38: replace || with && in parse_iso
#247 | crates/trellis-queue/src/stamp.rs:99:22: replace || with && in parse_iso
#247 | crates/trellis-queue/src/stamp.rs:105:58: replace || with && in parse_iso
#247 | crates/trellis-queue/src/stamp.rs:105:68: replace || with && in parse_iso
#247 | crates/trellis-queue/src/stamp.rs:105:79: replace || with && in parse_iso
#247 | crates/trellis-queue/src/stamp.rs:105:32: replace || with && in parse_iso
#247 | crates/trellis-queue/src/stamp.rs:105:86: replace > with == in parse_iso
#247 | crates/trellis-queue/src/stamp.rs:105:86: replace > with >= in parse_iso
```

</details>

<details>
<summary>trellis-cli: 407 survivors</summary>

#248's first pass (`c0fc32f`) gave its 119 dispositions in that commit's
message, not on these lines. The second pass marks its own here: each
mutant was applied by hand to the code at its base, the named test run
with `cargo test -p trellis-cli` and seen to fail, and the source restored;
neither cargo-mutants nor the workspace suite was run. Line numbers are
`989b2a2`'s; every work.rs mutant was still present, at a moved line, and
process_agent.rs's and services.rs's too. Pass 2 covers those three files
(112 lines): 89 killed, 3 equivalent, 3 unexercisable (a pty, a SIGINT
this Windows host cannot send, and one failure with no deterministic
trigger), and 17 not examined. Those 17 are
`#[cfg(unix)]` code that cargo-mutants mutated on a Windows host where it
is never compiled, so they survive there by construction. No bug was
found. A side finding, not a survivor: `queue list --priority` alone still
lists unreadable task files, which `--status`, `--label` and `--assignee`
hide.

The third pass marks its own the same way, over session.rs (40 lines),
the 29 main.rs lines the first pass left (that code now lives in
run_cmd.rs, doctor_cmd.rs and index_cmd.rs), and vault_cmd.rs (18, beside
the first pass's no-longer-applicable 189). session.rs: 32 killed, 1
equivalent, 2 no longer applicable (the line arithmetic moved to
`run_cmd::line_of`), 5 unexercisable (a terminal, which a pipe never is).
main.rs: 23 killed, 1 equivalent, 5 unexercisable (a terminal, a Ctrl-C,
a rollback that fails, an ERROR event on demand). vault_cmd.rs: 18 killed.
Two were already killed by a test in the suite at this pass's base; those
lines say so. No bug was found.

The fourth pass marks its own the same way, over the 89 lines no pass had
examined: orchestrate.rs 17, session_extra.rs 16, tooling.rs 10,
context_files.rs 8, doctor_extra.rs 7 (services_line), templates.rs 4,
tui_bridge.rs 3 (one already killed by the #243 budget test), rpc.rs 2 and
ask.rs 1, all killed; liveness.rs 11 (4 killed, 5 unix-only: `probe_ps`
runs only on a Unix that is not Linux, 2 unexercisable); tui_run.rs 6 (1
killed, 5 unexercisable behind the terminal check); image_input.rs 4 (3
killed, 1 unexercisable). 76 killed in all. Lines in tooling.rs,
context_files.rs, tui_run.rs and tui_bridge.rs had moved; each mutant was
found at its new line. No bug was found. The `#248 |` lines still open
below are the first pass's 119, whose dispositions are in `c0fc32f`'s
message.

```
unexercisable (#248): differs only for ERROR-level events (an empty RUST_LOG parsed as a directive is ERROR everywhere), and trellis's three are not reachable on demand: an orchestrator task gone, or a failed rollback of an interrupted plan | crates/trellis-cli/src/main.rs:401:18: replace match guard s.trim().is_empty() with false in main
#248 | crates/trellis-cli/src/main.rs:507:8: delete ! in main
unexercisable (#248): bare `trellis` with exactly one of stdin and stdout a terminal; every test here runs both as pipes, where `&&` and `||` agree, and a terminal needs a pty | crates/trellis-cli/src/main.rs:527:43: replace && with || in main
#248 | crates/trellis-cli/src/main.rs:585:17: delete match arm (true, _) in main
#248 | crates/trellis-cli/src/main.rs:611:21: delete field skills from struct output::RunExtras expression in main
#248 | crates/trellis-cli/src/main.rs:833:5: replace print_actions with ()
#248 | crates/trellis-cli/src/main.rs:860:12: delete ! in print_actions
#248 | crates/trellis-cli/src/main.rs:937:21: replace match guard m.task_mentions with true in cmd_why
#248 | crates/trellis-cli/src/main.rs:1090:13: replace > with == in pull_model
#248 | crates/trellis-cli/src/main.rs:1090:13: replace > with < in pull_model
#248 | crates/trellis-cli/src/main.rs:1090:13: replace > with >= in pull_model
#248 | crates/trellis-cli/src/main.rs:1220:9: replace <impl trellis_loop::Model for SeesDone<'_, M>>::tokens_used -> u64 with 0
#248 | crates/trellis-cli/src/main.rs:1220:9: replace <impl trellis_loop::Model for SeesDone<'_, M>>::tokens_used -> u64 with 1
#248 | crates/trellis-cli/src/main.rs:1224:9: replace <impl trellis_loop::Model for SeesDone<'_, M>>::prompt_bytes -> Option<usize> with None
#248 | crates/trellis-cli/src/main.rs:1224:9: replace <impl trellis_loop::Model for SeesDone<'_, M>>::prompt_bytes -> Option<usize> with Some(0)
#248 | crates/trellis-cli/src/main.rs:1224:9: replace <impl trellis_loop::Model for SeesDone<'_, M>>::prompt_bytes -> Option<usize> with Some(1)
#248 | crates/trellis-cli/src/main.rs:1228:9: replace <impl trellis_loop::Model for SeesDone<'_, M>>::compactions -> &[trellis_loop::Compacted] with Vec::leak(Vec::new())
killed (#248) by tests/prompt_tokens.rs a_local_runtime_s_compaction_reaches_the_trail_and_the_report | crates/trellis-cli/src/main.rs:1293:9: replace Driver::compactions -> Vec<trellis_loop::Compacted> with vec![]
#248 | crates/trellis-cli/src/main.rs:1303:9: replace Driver::requested_compactions -> Vec<trellis_loop::RequestedCompaction> with vec![]
unexercisable (#248): needs exactly one of stdin and stdout to be a terminal; every test here runs both as pipes, where `&&` and `||` agree | crates/trellis-cli/src/main.rs:1407:43: replace && with || in Human::terminal
killed (#248) by tooling::run_tests::a_custom_tool_s_write_to_an_ignored_file_is_undone_by_the_rollback | crates/trellis-cli/src/main.rs:1972:9: replace || with && in run_body
killed (#248) by tooling::run_tests::a_custom_tool_s_write_to_an_ignored_file_is_undone_by_the_rollback | crates/trellis-cli/src/main.rs:1971:12: delete ! in run_body
killed (#248) by tests/cli.rs a_run_with_only_skipped_skills_still_says_so | crates/trellis-cli/src/main.rs:2019:9: replace || with && in run_body
killed (#248) by tooling::run_tests::before_run_is_reported_only_when_it_adds_context_and_its_failure_is_said | crates/trellis-cli/src/main.rs:2195:13: delete match arm Some(Err(e)) in run_body
killed (#248) by tooling::run_tests::before_run_is_reported_only_when_it_adds_context_and_its_failure_is_said | crates/trellis-cli/src/main.rs:2183:31: replace match guard !text.trim().is_empty() with true in run_body
killed (#248) by tooling::run_tests::before_run_is_reported_only_when_it_adds_context_and_its_failure_is_said | crates/trellis-cli/src/main.rs:2183:31: replace match guard !text.trim().is_empty() with false in run_body
killed (#248) by tooling::run_tests::before_run_is_reported_only_when_it_adds_context_and_its_failure_is_said | crates/trellis-cli/src/main.rs:2183:31: delete ! in run_body
unexercisable (#248): reached only on a Ctrl-C mid-run with hooks configured and no `after_run`; interrupt.rs's tests send SIGINT on unix only, and this Windows host cannot send one | crates/trellis-cli/src/main.rs:2339:32: replace match guard h.config().after_run.is_some() with true in run_body
equivalent (#248): the arm is reached only when the two fingerprints (tree ids) differ, and `git diff-tree -r --name-only` between two different trees always names a path | crates/trellis-cli/src/main.rs:2426:38: replace match guard !paths.is_empty() with true in run_body
killed (#248) by tests/cli.rs a_failed_check_s_output_is_cut_at_twenty_four_lines_and_says_what_it_cut | crates/trellis-cli/src/main.rs:2547:24: replace > with == in run_body
killed (#248) by tests/cli.rs a_converged_run_names_the_ignored_files_it_changed_and_says_nothing_when_none | crates/trellis-cli/src/main.rs:2661:30: replace match guard c.is_empty() with true in run_body
killed (#248) by tests/cli.rs a_converged_run_names_the_ignored_files_it_changed_and_says_nothing_when_none | crates/trellis-cli/src/main.rs:2661:30: replace match guard c.is_empty() with false in run_body
killed (#248) by tests/cli.rs a_failed_check_s_output_is_cut_at_twenty_four_lines_and_says_what_it_cut | crates/trellis-cli/src/main.rs:2547:24: replace > with >= in run_body
unexercisable (#248): needs a run that asked a person and whose rollback then failed; no deterministic way to make `reset --hard` and `clean` fail after the snapshot was found (pass 2 met the same) | crates/trellis-cli/src/main.rs:2756:81: replace match guard restored with true in run_body
killed (#248) by run_cmd::approval_tests::a_queued_run_that_asks_ends_as_asked_with_the_question | crates/trellis-cli/src/main.rs:2756:81: replace match guard restored with false in run_body
#248 | crates/trellis-cli/src/main.rs:2854:22: replace > with == in unknown_run_id
#248 | crates/trellis-cli/src/main.rs:2854:22: replace > with < in unknown_run_id
#248 | crates/trellis-cli/src/main.rs:2854:22: replace > with >= in unknown_run_id
#248 | crates/trellis-cli/src/main.rs:2894:25: replace && with || in <impl Drop for RollbackOnPanic<'_>>::drop
#248 | crates/trellis-cli/src/main.rs:3177:16: delete ! in rollback_and_report
killed (#248) by run_cmd::audit_mode_tests::a_coverage_file_with_no_files_measures_nothing | crates/trellis-cli/src/main.rs:3227:28: replace || with && in audit_modes
killed (#248) by run_cmd::audit_mode_tests::a_covered_call_at_column_zero_is_verified_not_reviewed (already in the suite) | crates/trellis-cli/src/main.rs:3223:5: replace audit_modes -> trellis_trail::ModeCounts with Default::default()
#248 | crates/trellis-cli/src/main.rs:3534:36: replace == with != in record_trail
#248 | crates/trellis-cli/src/main.rs:3539:44: replace == with != in record_trail
#248 | crates/trellis-cli/src/main.rs:3557:41: replace > with == in record_trail
#248 | crates/trellis-cli/src/main.rs:3557:41: replace > with >= in record_trail
#248 | crates/trellis-cli/src/main.rs:3563:41: replace > with == in record_trail
#248 | crates/trellis-cli/src/main.rs:3563:41: replace > with >= in record_trail
killed (#248) by session::tests::radius_takes_a_name_qualified_by_only_its_last_modules | crates/trellis-cli/src/main.rs:3722:17: replace || with && in resolve_symbol
#248 | crates/trellis-cli/src/main.rs:4040:22: replace > with >= in render_backstop
#248 | crates/trellis-cli/src/main.rs:4192:39: replace && with || in graph_lines
#248 | crates/trellis-cli/src/main.rs:4192:30: replace == with != in graph_lines
#248 | crates/trellis-cli/src/main.rs:4192:26: replace + with * in graph_lines
#248 | crates/trellis-cli/src/main.rs:4192:56: replace == with != in graph_lines
#248 | crates/trellis-cli/src/main.rs:4198:26: replace > with >= in graph_lines
#248 | crates/trellis-cli/src/main.rs:4206:22: replace > with == in graph_lines
#248 | crates/trellis-cli/src/main.rs:4206:22: replace > with >= in graph_lines
#248 | crates/trellis-cli/src/main.rs:4252:5: replace codebase_lines -> Vec<String> with vec![]
#248 | crates/trellis-cli/src/main.rs:4252:5: replace codebase_lines -> Vec<String> with vec!["xyzzy".into()]
#248 | crates/trellis-cli/src/main.rs:4262:5: replace truncate_module -> String with String::new()
#248 | crates/trellis-cli/src/main.rs:4252:5: replace codebase_lines -> Vec<String> with vec![String::new()]
#248 | crates/trellis-cli/src/main.rs:4262:5: replace truncate_module -> String with "xyzzy".into()
#248 | crates/trellis-cli/src/main.rs:4263:10: replace <= with > in truncate_module
#248 | crates/trellis-cli/src/main.rs:4268:33: replace - with / in truncate_module
#248 | crates/trellis-cli/src/main.rs:4268:33: replace - with + in truncate_module
killed (#248) by tests/cli.rs doctor_reports_one_slot_as_single_stream_and_two_as_measured_concurrency | crates/trellis-cli/src/main.rs:4356:36: replace match guard n > 1 with true in cmd_doctor
killed (#248) by tests/cli.rs doctor_reports_one_slot_as_single_stream_and_two_as_measured_concurrency | crates/trellis-cli/src/main.rs:4356:38: replace > with == in cmd_doctor
killed (#248) by tests/cli.rs doctor_reports_one_slot_as_single_stream_and_two_as_measured_concurrency | crates/trellis-cli/src/main.rs:4356:36: replace match guard n > 1 with false in cmd_doctor
killed (#248) by tests/cli.rs doctor_reports_one_slot_as_single_stream_and_two_as_measured_concurrency | crates/trellis-cli/src/main.rs:4356:38: replace > with < in cmd_doctor
killed (#248) by tests/cli.rs doctor_reports_one_slot_as_single_stream_and_two_as_measured_concurrency | crates/trellis-cli/src/main.rs:4356:38: replace > with >= in cmd_doctor
killed (#248) by doctor_cmd::doctor_tests::coverage_is_available_only_when_the_test_runner_offers_cov | crates/trellis-cli/src/main.rs:4675:5: replace coverage_available -> bool with true
killed (#248) by doctor_cmd::doctor_tests::coverage_is_available_only_when_the_test_runner_offers_cov | crates/trellis-cli/src/main.rs:4675:5: replace coverage_available -> bool with false
#248 | crates/trellis-cli/src/main.rs:4782:18: replace match guard c.mode == config::ControlMode::Approve with true in repo_lines
#248 | crates/trellis-cli/src/main.rs:4782:18: replace match guard c.mode == config::ControlMode::Approve with false in repo_lines
#248 | crates/trellis-cli/src/main.rs:4782:25: replace == with != in repo_lines
killed (#248) by ask::tests::the_inventory_reaches_the_prompt_and_says_what_it_does_not_cover | crates/trellis-cli/src/ask.rs:116:19: replace > with >= in prompt_for
#248 | crates/trellis-cli/src/config.rs:453:27: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in RepoConfig::prompt_raw_in
#248 | crates/trellis-cli/src/config.rs:508:21: delete match arm "metered" in RepoConfig::budget
#248 | crates/trellis-cli/src/config.rs:509:21: delete match arm "local" in RepoConfig::budget
#248 | crates/trellis-cli/src/hosted.rs:213:54: replace || with && in origin
#248 | crates/trellis-cli/src/hosted.rs:325:5: replace thinking_ignored -> Option<String> with None
#248 | crates/trellis-cli/src/hosted.rs:325:5: replace thinking_ignored -> Option<String> with Some(String::new())
#248 | crates/trellis-cli/src/hosted.rs:325:5: replace thinking_ignored -> Option<String> with Some("xyzzy".into())
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with (String::new(), String::new(), vec![])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with (String::new(), String::new(), vec![String::new()])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with (String::new(), String::new(), vec!["xyzzy".into()])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with (String::new(), "xyzzy".into(), vec![])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with (String::new(), "xyzzy".into(), vec![String::new()])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with (String::new(), "xyzzy".into(), vec!["xyzzy".into()])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with ("xyzzy".into(), String::new(), vec![])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with ("xyzzy".into(), String::new(), vec![String::new()])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with ("xyzzy".into(), String::new(), vec!["xyzzy".into()])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with ("xyzzy".into(), "xyzzy".into(), vec![])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with ("xyzzy".into(), "xyzzy".into(), vec![String::new()])
#248 | crates/trellis-cli/src/hosted.rs:339:5: replace doctor -> (String, String, Vec<String>) with ("xyzzy".into(), "xyzzy".into(), vec!["xyzzy".into()])
#248 | crates/trellis-cli/src/hosted.rs:361:42: delete ! in doctor
#248 | crates/trellis-cli/src/hosted.rs:361:39: replace && with || in doctor
#248 | crates/trellis-cli/src/hosted.rs:373:5: replace compaction_line -> Option<String> with None
#248 | crates/trellis-cli/src/hosted.rs:373:5: replace compaction_line -> Option<String> with Some(String::new())
#248 | crates/trellis-cli/src/hosted.rs:373:5: replace compaction_line -> Option<String> with Some("xyzzy".into())
#248 | crates/trellis-cli/src/hosted.rs:413:5: replace requested_compaction_line -> Option<String> with None
#248 | crates/trellis-cli/src/hosted.rs:413:5: replace requested_compaction_line -> Option<String> with Some(String::new())
#248 | crates/trellis-cli/src/hosted.rs:413:5: replace requested_compaction_line -> Option<String> with Some("xyzzy".into())
killed (#248) by image_input::tests::an_image_of_exactly_the_cap_is_read_whole_and_one_byte_more_is_refused | crates/trellis-cli/src/image_input.rs:127:19: replace > with == in read_one
killed (#248) by image_input::tests::an_image_of_exactly_the_cap_is_read_whole_and_one_byte_more_is_refused | crates/trellis-cli/src/image_input.rs:127:19: replace > with >= in read_one
killed (#248) by image_input::tests::an_image_of_exactly_the_cap_is_read_whole_and_one_byte_more_is_refused | crates/trellis-cli/src/image_input.rs:132:53: replace + with - in read_one
unexercisable (#248): take(MAX) and take(MAX + 1) differ only for a file that grows past the cap between the metadata check and the read, a race with no hook to drive it | crates/trellis-cli/src/image_input.rs:132:53: replace + with * in read_one
#248 | crates/trellis-cli/src/install.rs:65:5: replace run -> Result<()> with Ok(())
#248 | crates/trellis-cli/src/install.rs:224:51: replace && with || in install
#248 | crates/trellis-cli/src/install.rs:338:79: replace || with && in split_spec
#248 | crates/trellis-cli/src/install.rs:346:5: replace unacceptable_source -> Option<String> with None
#248 | crates/trellis-cli/src/install.rs:404:23: replace || with && in bad_component
#248 | crates/trellis-cli/src/install.rs:404:36: replace > with == in bad_component
#248 | crates/trellis-cli/src/install.rs:404:36: replace > with >= in bad_component
#248 | crates/trellis-cli/src/install.rs:444:67: replace || with && in skill_name_ok
#248 | crates/trellis-cli/src/install.rs:518:30: replace match guard file.ends_with(".md") with true in why_skipped
#248 | crates/trellis-cli/src/install.rs:543:19: replace > with >= in check_bounds
#248 | crates/trellis-cli/src/install.rs:549:50: replace > with == in check_bounds
#248 | crates/trellis-cli/src/install.rs:549:50: replace > with >= in check_bounds
#248 | crates/trellis-cli/src/install.rs:557:14: replace > with == in check_bounds
#248 | crates/trellis-cli/src/install.rs:557:14: replace > with >= in check_bounds
#248 | crates/trellis-cli/src/install.rs:636:41: replace || with && in resolve
#248 | crates/trellis-cli/src/install.rs:773:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in load
#248 | crates/trellis-cli/src/install.rs:801:38: replace || with && in remove_tree
killed (#248) by liveness::tests::the_host_is_the_name_the_platform_records_for_this_machine | crates/trellis-cli/src/liveness.rs:29:9: replace <impl Liveness for SystemLiveness>::host -> Option<String> with Some(String::new())
killed (#248) by liveness::tests::the_host_is_the_name_the_platform_records_for_this_machine | crates/trellis-cli/src/liveness.rs:29:9: replace <impl Liveness for SystemLiveness>::host -> Option<String> with Some("xyzzy".into())
unexercisable (#248): needs a /proc/<pid>/stat read that fails other than NotFound (a holder exiting between open and read); none happens on demand, and `/proc` stat files are world-readable | crates/trellis-cli/src/liveness.rs:59:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in probe_proc
killed (#248) by liveness::tests::a_pid_with_no_proc_entry_is_gone | crates/trellis-cli/src/liveness.rs:59:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with false in probe_proc
killed (#248) by liveness::tests::a_pid_with_no_proc_entry_is_gone | crates/trellis-cli/src/liveness.rs:59:28: replace == with != in probe_proc
unix-only (#248): probe_ps is chosen only on a Unix that is not Linux (macOS, BSD); this host is Windows and CI's runner Linux, and a direct call here would test whatever `ps` PATH holds (Git for Windows' takes no -o) | crates/trellis-cli/src/liveness.rs:123:28: replace match guard !out.trim().is_empty() with true in probe_ps
unix-only (#248): probe_ps is chosen only on a Unix that is not Linux (macOS, BSD); this host is Windows and CI's runner Linux, and a direct call here would test whatever `ps` PATH holds (Git for Windows' takes no -o) | crates/trellis-cli/src/liveness.rs:123:28: replace match guard !out.trim().is_empty() with false in probe_ps
unix-only (#248): probe_ps is chosen only on a Unix that is not Linux (macOS, BSD); this host is Windows and CI's runner Linux, and a direct call here would test whatever `ps` PATH holds (Git for Windows' takes no -o) | crates/trellis-cli/src/liveness.rs:126:29: replace match guard out.trim().is_empty() with false in probe_ps
unix-only (#248): probe_ps is chosen only on a Unix that is not Linux (macOS, BSD); this host is Windows and CI's runner Linux, and a direct call here would test whatever `ps` PATH holds (Git for Windows' takes no -o) | crates/trellis-cli/src/liveness.rs:126:29: replace match guard out.trim().is_empty() with true in probe_ps
unix-only (#248): probe_ps is chosen only on a Unix that is not Linux (macOS, BSD); this host is Windows and CI's runner Linux, and a direct call here would test whatever `ps` PATH holds (Git for Windows' takes no -o) | crates/trellis-cli/src/liveness.rs:123:28: delete ! in probe_ps
unexercisable (#248): needs a probe child that runs past PROBE_BOUND, a 20 s const; a test would block for it | crates/trellis-cli/src/liveness.rs:146:25: replace match guard started.elapsed() >= PROBE_BOUND with false in bounded
killed (#248) by tests/orchestration.rs a_routine_may_assign_its_task_to_a_profile_and_not_to_a_missing_one | crates/trellis-cli/src/orchestrate.rs:97:16: delete ! in load_routines
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:131:5: replace limit_text -> String with String::new()
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:131:5: replace limit_text -> String with "xyzzy".into()
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:164:20: replace match guard p.file.paused with true in state_text
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:164:20: replace match guard p.file.paused with false in state_text
killed (#248) by tests/orchestration.rs a_budget_hard_stop_pauses_the_agent_and_an_approved_raise_resumes_it | crates/trellis-cli/src/orchestrate.rs:166:13: delete match arm Some(PausedBy::Budget) in state_text
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:201:25: replace match guard !p.file.pass_env.is_empty() with true in runs_with
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:309:40: replace == with != in agents
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:378:41: replace > with == in agents
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:378:41: replace > with < in agents
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:378:41: replace > with >= in agents
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:385:30: replace > with == in agents
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:385:30: replace > with < in agents
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:385:30: replace > with >= in agents
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:450:25: replace match guard !p.file.paused with true in agents
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:450:25: replace match guard !p.file.paused with false in agents
killed (#248) by tests/orchestration.rs agents_show_and_resume_say_which_pause_holds_and_what_the_trails_lacked | crates/trellis-cli/src/orchestrate.rs:450:25: delete ! in agents
killed (#248) by process_agent::tests::a_path_deny_write_refuses_is_refused_once_not_again_by_allow_write | crates/trellis-cli/src/process_agent.rs:135:51: replace || with && in Judge::governance
killed (#248) by process_agent::tests::each_refusal_is_given_as_its_own_reason | crates/trellis-cli/src/process_agent.rs:401:8: delete ! in run
killed (#248) by process_agent::tests::a_change_that_cannot_be_read_or_rolled_back_is_not_restored | crates/trellis-cli/src/process_agent.rs:414:62: replace && with || in run
killed (#248) by process_agent::tests::a_command_that_changed_nothing_is_not_audited | crates/trellis-cli/src/process_agent.rs:444:42: replace && with || in run
killed (#248) by process_agent::tests::a_timed_out_command_says_it_timed_out | crates/trellis-cli/src/process_agent.rs:505:13: delete match arm Ended::TimedOut in run
killed (#248) by process_agent::tests::an_interrupted_command_ends_as_interrupted_and_says_so | crates/trellis-cli/src/process_agent.rs:506:13: delete match arm Ended::Interrupted in run
killed (#248) by process_agent::tests::each_refusal_is_given_as_its_own_reason | crates/trellis-cli/src/process_agent.rs:507:18: replace match guard !git_changed.is_empty() with false in run
unexercisable (#248): needs the ignored-file listing to fail after the command while `git diff` on the same repository succeeds (a broken index fails both, and returns earlier); no deterministic way found | crates/trellis-cli/src/process_agent.rs:514:18: replace match guard ignored_unread.is_some() with false in run
killed (#248) by process_agent::tests::each_refusal_is_given_as_its_own_reason | crates/trellis-cli/src/process_agent.rs:523:18: replace match guard !denied.is_empty() with false in run
killed (#248) by process_agent::tests::an_interrupted_command_ends_as_interrupted_and_says_so | crates/trellis-cli/src/process_agent.rs:572:9: delete match arm Ended::Interrupted in run
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; the stdin, interrupt and timeout tests in process_agent::tests reach it on unix, not run there for this pass | crates/trellis-cli/src/process_agent.rs:640:9: replace Running::spawn -> std::io::Result<Running> with Ok(Default::default())
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; the stdin, interrupt and timeout tests in process_agent::tests reach it on unix, not run there for this pass | crates/trellis-cli/src/process_agent.rs:644:9: replace Running::take_stdin -> Option<std::process::ChildStdin> with None
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; the stdin, interrupt and timeout tests in process_agent::tests reach it on unix, not run there for this pass | crates/trellis-cli/src/process_agent.rs:644:9: replace Running::take_stdin -> Option<std::process::ChildStdin> with Some(Default::default())
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; the stdin, interrupt and timeout tests in process_agent::tests reach it on unix, not run there for this pass | crates/trellis-cli/src/process_agent.rs:648:9: replace Running::try_wait -> std::io::Result<Option<std::process::ExitStatus>> with Ok(None)
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; the stdin, interrupt and timeout tests in process_agent::tests reach it on unix, not run there for this pass | crates/trellis-cli/src/process_agent.rs:648:9: replace Running::try_wait -> std::io::Result<Option<std::process::ExitStatus>> with Ok(Some(Default::default()))
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; the stdin, interrupt and timeout tests in process_agent::tests reach it on unix, not run there for this pass | crates/trellis-cli/src/process_agent.rs:659:9: replace Running::end with ()
killed (#248) by process_agent::tests::the_task_text_reaches_the_command_on_stdin | crates/trellis-cli/src/process_agent.rs:709:9: replace Running::take_stdin -> Option<std::fs::File> with None
equivalent (#248): `end` is `finish` and a bounded reap; the loop calls `finish` again as soon as it breaks, so the job is ended either way and only the reap of an already-killed process is lost | crates/trellis-cli/src/process_agent.rs:721:9: replace Running::end with ()
killed (#248) by process_agent::tests::a_timed_out_command_is_ended_before_it_can_write_again | crates/trellis-cli/src/process_agent.rs:731:9: replace Running::finish with ()
killed (#248) by process_agent::tests::the_trail_says_whether_the_command_wrote_and_what_it_did_to_ignored_files | crates/trellis-cli/src/process_agent.rs:815:24: delete ! in write_trail
killed (#248) by process_agent::tests::the_trail_says_whether_the_command_wrote_and_what_it_did_to_ignored_files | crates/trellis-cli/src/process_agent.rs:822:21: delete ! in write_trail
killed (#248) by services::tests::a_record_is_read_up_to_its_bound_and_refused_by_size_past_it | crates/trellis-cli/src/services.rs:40:34: replace * with +
killed (#248) by services::tests::a_name_that_cannot_be_replaced_is_said_to_be_that | crates/trellis-cli/src/services.rs:256:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in create_fresh
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:277:9: replace Proc::spawn -> Result<(Proc, u32)> with Ok((Default::default(), 0))
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:277:9: replace Proc::spawn -> Result<(Proc, u32)> with Ok((Default::default(), 1))
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:287:9: replace Proc::stop -> bool with true
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:287:9: replace Proc::stop -> bool with false
equivalent (#248): every caller drops the `Proc` right after `stop`, and its KillOnDrop and kill-on-close job end the tree then; the mutant only removes the record a moment before the tree is gone instead of after | crates/trellis-cli/src/services.rs:333:9: replace Proc::stop -> bool with true
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:349:5: replace reap -> bool with true
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:349:5: replace reap -> bool with false
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:349:43: replace + with - in reap
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:349:43: replace + with * in reap
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:354:25: replace match guard std::time::Instant::now() >= until with true in reap
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:354:25: replace match guard std::time::Instant::now() >= until with false in reap
not examined (#248): `#[cfg(unix)]` code, which cargo-mutants mutates but the Windows host it ran on never compiles, so it survives there by construction; not run on unix for this pass | crates/trellis-cli/src/services.rs:354:51: replace >= with < in reap
killed (#248) by services::tests::no_services_directory_is_nothing_and_one_that_cannot_be_read_is_said | crates/trellis-cli/src/services.rs:369:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in outliving
killed (#248) by services::tests::no_services_directory_is_nothing_and_one_that_cannot_be_read_is_said | crates/trellis-cli/src/services.rs:369:28: replace == with != in outliving
killed (#248) by services::tests::no_services_directory_is_nothing_and_one_that_cannot_be_read_is_said | crates/trellis-cli/src/services.rs:369:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with false in outliving
killed (#248) by services::tests::a_record_is_read_up_to_its_bound_and_refused_by_size_past_it | crates/trellis-cli/src/services.rs:410:29: replace + with - in read_record
killed (#248) by services::tests::a_record_is_read_up_to_its_bound_and_refused_by_size_past_it | crates/trellis-cli/src/services.rs:413:26: replace > with >= in read_record
killed (#248) by services::tests::a_record_is_read_up_to_its_bound_and_refused_by_size_past_it | crates/trellis-cli/src/services.rs:410:29: replace + with * in read_record
killed (#248) by services::tests::a_record_is_read_up_to_its_bound_and_refused_by_size_past_it | crates/trellis-cli/src/services.rs:413:26: replace > with == in read_record
killed (#248) by session::tests::stdout_reports_every_byte_it_was_given | crates/trellis-cli/src/session.rs:35:9: replace <impl std::io::Write for Stdout>::write -> std::io::Result<usize> with Ok(0)
killed (#248) by session::tests::stdout_reports_every_byte_it_was_given | crates/trellis-cli/src/session.rs:35:9: replace <impl std::io::Write for Stdout>::write -> std::io::Result<usize> with Ok(1)
unexercisable (#248): stdout flushes itself at each newline and at exit, so a missing flush shows only as a partial line late on a terminal, the one place `Stdout` is used (a steered run needs stdin and stdout both terminals) | crates/trellis-cli/src/session.rs:38:9: replace <impl std::io::Write for Stdout>::flush -> std::io::Result<()> with Ok(())
unexercisable (#248): the pending count is only in the prompt, which rustyline does not print when stdin is not a terminal, and the session prints itself only after a steered run, which needs a terminal; no pty here | crates/trellis-cli/src/session.rs:208:37: replace > with == in Session::run
unexercisable (#248): the pending count is only in the prompt, which rustyline does not print when stdin is not a terminal, and the session prints itself only after a steered run, which needs a terminal; no pty here | crates/trellis-cli/src/session.rs:208:37: replace > with < in Session::run
unexercisable (#248): the pending count is only in the prompt, which rustyline does not print when stdin is not a terminal, and the session prints itself only after a steered run, which needs a terminal; no pty here | crates/trellis-cli/src/session.rs:208:37: replace > with >= in Session::run
killed (#248) by session::tests::slash_models_lists_rather_than_being_an_unknown_command | crates/trellis-cli/src/session.rs:379:13: delete match arm "/models" in Session::dispatch
killed (#248) by session::tests::slash_model_alone_shows_the_model_and_changes_nothing | crates/trellis-cli/src/session.rs:318:25: replace match guard rest.is_empty() with false in Session::dispatch
equivalent (#248): `rest` is trimmed before the split, so whatever follows its first whitespace ends in a non-whitespace character and `task.trim()` is never empty | crates/trellis-cli/src/session.rs:385:39: replace match guard !task.trim().is_empty() with true in Session::dispatch
killed (#248) by session::tests::slash_skill_runs_the_task_with_the_skill_and_refuses_one_it_cannot_find | crates/trellis-cli/src/session.rs:385:39: replace match guard !task.trim().is_empty() with false in Session::dispatch
killed (#248) by session::tests::slash_skill_runs_the_task_with_the_skill_and_refuses_one_it_cannot_find | crates/trellis-cli/src/session.rs:385:39: delete ! in Session::dispatch
killed (#248) by tests/cli.rs a_runtime_that_is_not_listening_is_said_at_open_and_at_model | crates/trellis-cli/src/session.rs:434:9: replace Session::warn_if_unreachable with ()
killed (#248) by tests/cli.rs a_runtime_that_is_not_listening_is_said_at_open_and_at_model | crates/trellis-cli/src/session.rs:468:9: replace Session::probe_model with ()
unexercisable (#248): needs exactly one of stdin and stdout to be a terminal; every test here runs both as pipes, where `&&` and `||` agree | crates/trellis-cli/src/session.rs:589:43: replace && with || in Session::run_task
killed (#248) by session::tests::slash_skill_runs_the_task_with_the_skill_and_refuses_one_it_cannot_find | crates/trellis-cli/src/session.rs:604:17: delete field skills from struct crate::output::RunExtras expression in Session::run_task
killed (#248) by session::tests::a_steered_run_carries_its_skill | crates/trellis-cli/src/session.rs:652:17: delete field skills from struct crate::output::RunExtras expression in Session::run_steered
killed (#248) by tests/hosted_keys.rs a_hosted_spec_typed_at_the_session_prompt_is_the_user_s_choice_for_the_run_too | crates/trellis-cli/src/session.rs:690:17: delete field model_from_user from struct crate::RunSeeding expression in Session::run_with
killed (#248) by session::tests::the_approval_prompt_names_unbound_calls_and_offers_the_choices | crates/trellis-cli/src/session.rs:853:26: replace > with == in Session::accept_with
killed (#248) by session::tests::the_approval_prompt_names_unbound_calls_and_offers_the_choices | crates/trellis-cli/src/session.rs:853:26: replace > with < in Session::accept_with
killed (#248) by session::tests::a_commit_with_no_doubt_carries_no_coverage_paragraph | crates/trellis-cli/src/session.rs:853:26: replace > with >= in Session::accept_with
killed (#248) by session::tests::the_approval_prompt_names_unbound_calls_and_offers_the_choices | crates/trellis-cli/src/session.rs:871:9: replace Session::print_coverage with ()
killed (#248) by session::tests::the_approval_prompt_names_a_file_it_never_indexed_without_calling_it_unbound | crates/trellis-cli/src/session.rs:873:27: replace > with == in Session::print_coverage
killed (#248) by session::tests::the_approval_prompt_names_a_file_it_never_indexed_without_calling_it_unbound | crates/trellis-cli/src/session.rs:873:27: replace > with < in Session::print_coverage
killed (#248) by session::tests::the_approval_prompt_names_unbound_calls_and_offers_the_choices | crates/trellis-cli/src/session.rs:873:27: replace > with >= in Session::print_coverage
killed (#248) by session::tests::the_approval_prompt_names_unbound_calls_and_offers_the_choices | crates/trellis-cli/src/session.rs:884:22: replace > with == in Session::print_coverage
killed (#248) by session::tests::the_approval_prompt_names_unbound_calls_and_offers_the_choices | crates/trellis-cli/src/session.rs:884:22: replace > with < in Session::print_coverage
killed (#248) by session::tests::the_approval_prompt_names_a_file_it_never_indexed_without_calling_it_unbound | crates/trellis-cli/src/session.rs:884:22: replace > with >= in Session::print_coverage
killed (#248) by session::tests::the_approval_prompt_says_what_a_passing_run_does_not_claim (already in the suite: the note now goes to the writer that test reads) | crates/trellis-cli/src/session.rs:908:9: replace Session::print_audit_mode with ()
killed (#248) by session::tests::the_approval_prompt_names_unbound_calls_and_offers_the_choices | crates/trellis-cli/src/session.rs:912:9: replace Session::print_choices with ()
killed (#248) by tests/cli.rs accept_reads_its_answer_from_the_next_line_of_input | crates/trellis-cli/src/session.rs:920:9: replace Session::ask_stdin -> Option<char> with None
killed (#248) by tests/cli.rs accept_reads_its_answer_from_the_next_line_of_input | crates/trellis-cli/src/session.rs:920:9: replace Session::ask_stdin -> Option<char> with Some(Default::default())
killed (#248) by tests/cli.rs accept_reads_its_answer_from_the_next_line_of_input | crates/trellis-cli/src/session.rs:924:56: replace == with != in Session::ask_stdin
killed (#248) by session::tests::inspect_lists_each_site_by_line_and_says_what_the_list_is_not | crates/trellis-cli/src/session.rs:942:9: replace Session::inspect_doubt with ()
not applicable (#248): the line number is now `run_cmd::line_of`, not arithmetic in `inspect_doubt`; inspect_lists_each_site_by_line_and_says_what_the_list_is_not asserts the line it gives | crates/trellis-cli/src/session.rs:946:81: replace + with - in Session::inspect_doubt
not applicable (#248): the line number is now `run_cmd::line_of`, not arithmetic in `inspect_doubt`; inspect_lists_each_site_by_line_and_says_what_the_list_is_not asserts the line it gives | crates/trellis-cli/src/session.rs:946:81: replace + with * in Session::inspect_doubt
killed (#248) by session::tests::inspect_shows_twenty_sites_and_counts_only_what_it_left_out | crates/trellis-cli/src/session.rs:954:26: replace > with == in Session::inspect_doubt
killed (#248) by session::tests::inspect_shows_twenty_sites_and_counts_only_what_it_left_out | crates/trellis-cli/src/session.rs:954:26: replace > with >= in Session::inspect_doubt
killed (#248) by session::tests::what_to_test_names_each_function_with_its_own_count | crates/trellis-cli/src/session.rs:975:9: replace Session::what_to_test with ()
killed (#248) by session::tests::what_to_test_names_each_function_with_its_own_count | crates/trellis-cli/src/session.rs:979:74: replace += with *= in Session::what_to_test
killed (#248) by tests/cli.rs a_session_keeps_its_history_outside_the_repository | crates/trellis-cli/src/session.rs:1075:5: replace history_path -> Option<PathBuf> with None
killed (#248) by tooling::run_tests::the_untrusted_line_counts_what_is_present_and_a_trusted_run_without_hooks_loads_none | crates/trellis-cli/src/tooling.rs:240:46: replace > with >= in prepare
killed (#248) by tooling::run_tests::the_untrusted_line_counts_what_is_present_and_a_trusted_run_without_hooks_loads_none | crates/trellis-cli/src/tooling.rs:250:29: replace > with >= in prepare
killed (#248) by tooling::run_tests::the_untrusted_line_counts_what_is_present_and_a_trusted_run_without_hooks_loads_none | crates/trellis-cli/src/tooling.rs:267:19: replace > with < in prepare
killed (#248) by tooling::run_tests::the_untrusted_line_counts_what_is_present_and_a_trusted_run_without_hooks_loads_none | crates/trellis-cli/src/tooling.rs:275:21: replace > with < in prepare
killed (#248) by tooling::run_tests::a_profile_may_keep_a_declared_custom_tool_by_name_and_not_a_typo | crates/trellis-cli/src/tooling.rs:301:64: replace == with != in check_profile_tools
killed (#248) by tooling::run_tests::a_profile_that_leaves_out_run_checks_is_told_it_was_kept | crates/trellis-cli/src/tooling.rs:334:12: delete ! in Tooling::narrow_for_profile
killed (#248) by tooling::run_tests::a_profile_that_leaves_out_run_checks_is_told_it_was_kept | crates/trellis-cli/src/tooling.rs:334:35: replace == with != in Tooling::narrow_for_profile
killed (#248) by tooling::run_tests::the_tools_line_names_auto_mode_s_bash_policy | crates/trellis-cli/src/tooling.rs:371:38: replace match guard b.unattended with false in Tooling::line
killed (#248) by tooling::run_tests::the_tools_line_names_auto_mode_s_bash_policy | crates/trellis-cli/src/tooling.rs:372:38: replace match guard b.allow.is_empty() with false in Tooling::line
killed (#248) by tooling::run_tests::after_run_is_told_whether_the_run_converged_and_why_not | crates/trellis-cli/src/tooling.rs:400:5: replace outcome_json -> serde_json::Value with Default::default()
killed (#248) by tui_bridge::tests::tui_bridge_budget_carries_the_runs_ceiling_and_its_elapsed_time (already in the suite, since #243) | crates/trellis-cli/src/tui_bridge.rs:45:13: delete match arm "prompt_bytes" in <impl Visit for TurnFields>::record_u64
killed (#248) by tui_bridge::tests::a_warning_whose_message_is_a_plain_string_field_still_reaches_the_transcript | crates/trellis-cli/src/tui_bridge.rs:56:13: delete match arm "message" in <impl Visit for TurnFields>::record_str
killed (#248) by tui_bridge::tests::a_check_kind_written_with_the_display_sigil_names_the_check | crates/trellis-cli/src/tui_bridge.rs:78:13: delete match arm "check_kind" in <impl Visit for TurnFields>::record_debug
killed (#248) by tui_run::tests::a_model_the_person_chose_in_the_tui_is_not_refused_as_the_repository_s | crates/trellis-cli/src/tui_run.rs:334:17: delete field model_from_user from struct crate::RunSeeding expression in run_for_tui
unexercisable (#248): differs only with exactly one of stdin and stdout a terminal; every test here runs both as pipes, where the mutant and the original both refuse, and a terminal needs a pty | crates/trellis-cli/src/tui_run.rs:408:40: replace || with && in run
unexercisable (#248): differs only with exactly one of stdin and stdout a terminal; every test here runs both as pipes, where the mutant and the original both refuse, and a terminal needs a pty | crates/trellis-cli/src/tui_run.rs:408:8: delete ! in run
unexercisable (#248): differs only with exactly one of stdin and stdout a terminal; every test here runs both as pipes, where the mutant and the original both refuse, and a terminal needs a pty | crates/trellis-cli/src/tui_run.rs:408:43: delete ! in run
unexercisable (#248): the status bar's first label, set only after `run` has found a terminal on both ends, which no test here has (a pty) | crates/trellis-cli/src/tui_run.rs:435:9: delete match arm Some("scripted") in run
unexercisable (#248): inside `run`'s task driver, reached only after the terminal check, which no test here passes (a pty) | crates/trellis-cli/src/tui_run.rs:515:24: delete ! in run
killed (#248) by tests/cli.rs vault_dates_says_there_is_no_file_and_seed_makes_one_dated_from_the_files | crates/trellis-cli/src/vault_cmd.rs:159:5: replace file_days -> FileDays with Default::default()
killed (#248) by tests/cli.rs vault_new_says_what_it_created | crates/trellis-cli/src/vault_cmd.rs:171:5: replace print_outcome -> Result<()> with Ok(())
killed (#248) by tests/cli.rs vault_check_lists_issues_only_when_there_are_some_and_names_the_fix_for_a_refusal | crates/trellis-cli/src/vault_cmd.rs:187:5: replace check -> Result<()> with Ok(())
#248 | crates/trellis-cli/src/vault_cmd.rs:189:8: delete ! in check
killed (#248) by tests/cli.rs vault_check_lists_issues_only_when_there_are_some_and_names_the_fix_for_a_refusal | crates/trellis-cli/src/vault_cmd.rs:197:8: delete ! in check
killed (#248) by tests/cli.rs vault_check_lists_issues_only_when_there_are_some_and_names_the_fix_for_a_refusal | crates/trellis-cli/src/vault_cmd.rs:216:16: delete ! in check
killed (#248) by tests/cli.rs vault_search_finds_a_body_word_and_reports_no_reduction_on_a_small_vault | crates/trellis-cli/src/vault_cmd.rs:228:5: replace search -> Result<()> with Ok(())
killed (#248) by tests/cli.rs vault_search_finds_a_body_word_and_reports_no_reduction_on_a_small_vault | crates/trellis-cli/src/vault_cmd.rs:230:8: delete ! in search
killed (#248) by tests/cli.rs vault_search_finds_a_body_word_and_reports_no_reduction_on_a_small_vault | crates/trellis-cli/src/vault_cmd.rs:232:15: delete ! in search
killed (#248) by tests/cli.rs vault_graph_lists_a_note_s_own_issues_and_no_other_note_s | crates/trellis-cli/src/vault_cmd.rs:246:5: replace graph -> Result<()> with Ok(())
killed (#248) by tests/cli.rs vault_graph_lists_a_note_s_own_issues_and_no_other_note_s | crates/trellis-cli/src/vault_cmd.rs:303:8: delete ! in graph
killed (#248) by tests/cli.rs vault_graph_lists_a_note_s_own_issues_and_no_other_note_s | crates/trellis-cli/src/vault_cmd.rs:310:5: replace mentions -> bool with true
killed (#248) by tests/cli.rs vault_graph_lists_a_note_s_own_issues_and_no_other_note_s | crates/trellis-cli/src/vault_cmd.rs:310:5: replace mentions -> bool with false
killed (#248) by tests/cli.rs vault_graph_lists_a_note_s_own_issues_and_no_other_note_s | crates/trellis-cli/src/vault_cmd.rs:311:85: replace == with != in mentions
killed (#248) by tests/cli.rs vault_graph_lists_a_note_s_own_issues_and_no_other_note_s | crates/trellis-cli/src/vault_cmd.rs:313:36: replace == with != in mentions
killed (#248) by tests/cli.rs vault_dates_says_there_is_no_file_and_seed_makes_one_dated_from_the_files | crates/trellis-cli/src/vault_cmd.rs:363:5: replace dates_cmd -> Result<()> with Ok(())
killed (#248) by tests/cli.rs vault_dates_says_there_is_no_file_and_seed_makes_one_dated_from_the_files | crates/trellis-cli/src/vault_cmd.rs:368:26: replace match guard !seed with false in dates_cmd
killed (#248) by tests/cli.rs vault_dates_says_there_is_no_file_and_seed_makes_one_dated_from_the_files | crates/trellis-cli/src/vault_cmd.rs:368:26: replace match guard !seed with true in dates_cmd
killed (#248) by tests/cli.rs vault_dates_says_there_is_no_file_and_seed_makes_one_dated_from_the_files | crates/trellis-cli/src/vault_cmd.rs:368:26: delete ! in dates_cmd
killed (#248) by work::queue_set_tests::a_change_that_ends_a_blocker_wakes_what_it_blocked | crates/trellis-cli/src/work.rs:198:5: replace reconcile -> Result<()> with Ok(())
killed (#248) by tests/orchestration.rs queue_list_filters_show_only_their_tasks_and_no_unreadable_file | crates/trellis-cli/src/work.rs:293:68: replace && with || in queue
killed (#248) by tests/orchestration.rs queue_list_filters_show_only_their_tasks_and_no_unreadable_file | crates/trellis-cli/src/work.rs:293:49: replace && with || in queue
killed (#248) by tests/orchestration.rs queue_list_filters_show_only_their_tasks_and_no_unreadable_file | crates/trellis-cli/src/work.rs:297:71: replace == with != in queue
killed (#248) by tests/orchestration.rs queue_list_filters_show_only_their_tasks_and_no_unreadable_file | crates/trellis-cli/src/work.rs:301:67: replace == with != in queue
killed (#248) by tests/orchestration.rs queue_list_filters_show_only_their_tasks_and_no_unreadable_file | crates/trellis-cli/src/work.rs:302:83: replace == with != in queue
killed (#248) by work::queue_set_tests::an_assignee_must_be_a_profile_in_the_repository | crates/trellis-cli/src/work.rs:348:20: delete ! in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:353:43: replace != with == in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:361:17: delete match arm (Some(Status::Blocked), Some("blockers")) in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:362:17: delete match arm (Some(Status::Blocked), Some("children")) in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:363:17: delete match arm (Some(Status::Blocked), Some(_)) in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:366:37: delete ! in queue
killed (#248) by work::queue_set_tests::labels_and_blockers_are_added_once_and_removed_by_name | crates/trellis-cli/src/work.rs:373:22: replace == with != in queue
killed (#248) by work::queue_set_tests::labels_and_blockers_are_added_once_and_removed_by_name | crates/trellis-cli/src/work.rs:376:20: delete ! in queue
killed (#248) by work::queue_set_tests::labels_and_blockers_are_added_once_and_removed_by_name | crates/trellis-cli/src/work.rs:388:24: delete ! in queue
killed (#248) by work::queue_set_tests::labels_and_blockers_are_added_once_and_removed_by_name | crates/trellis-cli/src/work.rs:394:43: replace != with == in queue
killed (#248) by work::queue_set_tests::labels_and_blockers_are_added_once_and_removed_by_name | crates/trellis-cli/src/work.rs:414:24: delete ! in queue
killed (#248) by work::queue_set_tests::labels_and_blockers_are_added_once_and_removed_by_name | crates/trellis-cli/src/work.rs:420:45: replace != with == in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:434:52: replace match guard t.blockers.is_empty() with true in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:434:52: replace match guard t.blockers.is_empty() with false in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:441:52: replace match guard children == 0 with true in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:441:52: replace match guard children == 0 with false in queue
killed (#248) by work::queue_set_tests::a_block_is_set_only_with_a_reason_that_holds_and_keeps_that_reason | crates/trellis-cli/src/work.rs:441:61: replace == with != in queue
killed (#248) by work::queue_set_tests::a_status_back_to_pending_clears_the_old_outcome_and_no_other_move_does | crates/trellis-cli/src/work.rs:454:52: replace && with || in queue
killed (#248) by work::queue_set_tests::a_status_back_to_pending_clears_the_old_outcome_and_no_other_move_does | crates/trellis-cli/src/work.rs:454:57: replace != with == in queue
killed (#248) by work::queue_set_tests::a_status_back_to_pending_clears_the_old_outcome_and_no_other_move_does | crates/trellis-cli/src/work.rs:454:34: replace == with != in queue
killed (#248) by work::queue_set_tests::only_new_unfinished_blockers_on_a_todo_task_block_it | crates/trellis-cli/src/work.rs:464:21: replace && with || in queue
killed (#248) by work::queue_set_tests::only_new_unfinished_blockers_on_a_todo_task_block_it | crates/trellis-cli/src/work.rs:463:21: replace && with || in queue
killed (#248) by work::queue_set_tests::only_new_unfinished_blockers_on_a_todo_task_block_it | crates/trellis-cli/src/work.rs:461:29: replace == with != in queue
killed (#248) by work::queue_set_tests::only_new_unfinished_blockers_on_a_todo_task_block_it | crates/trellis-cli/src/work.rs:462:21: replace && with || in queue
killed (#248) by work::queue_set_tests::only_new_unfinished_blockers_on_a_todo_task_block_it | crates/trellis-cli/src/work.rs:462:24: delete ! in queue
killed (#248) by work::queue_set_tests::only_new_unfinished_blockers_on_a_todo_task_block_it | crates/trellis-cli/src/work.rs:463:39: replace == with != in queue
killed (#248) by work::queue_set_tests::only_new_unfinished_blockers_on_a_todo_task_block_it | crates/trellis-cli/src/work.rs:464:24: delete ! in queue
killed (#248) by tests/orchestration.rs export_says_how_many_claimed_tasks_it_left_out_and_nothing_when_none | crates/trellis-cli/src/work.rs:616:34: replace > with == in queue
killed (#248) by tests/orchestration.rs export_says_how_many_claimed_tasks_it_left_out_and_nothing_when_none | crates/trellis-cli/src/work.rs:616:34: replace > with < in queue
killed (#248) by tests/orchestration.rs export_says_how_many_claimed_tasks_it_left_out_and_nothing_when_none | crates/trellis-cli/src/work.rs:616:34: replace > with >= in queue
killed (#248) by work::list_line_tests::attempts_blockers_and_notes_are_shown_when_there_are_any | crates/trellis-cli/src/work.rs:716:19: replace > with == in list_line
killed (#248) by work::list_line_tests::attempts_blockers_and_notes_are_shown_when_there_are_any | crates/trellis-cli/src/work.rs:716:19: replace > with < in list_line
killed (#248) by work::list_line_tests::attempts_blockers_and_notes_are_shown_when_there_are_any | crates/trellis-cli/src/work.rs:716:19: replace > with >= in list_line
killed (#248) by work::list_line_tests::attempts_blockers_and_notes_are_shown_when_there_are_any | crates/trellis-cli/src/work.rs:738:8: delete ! in list_line
killed (#248) by work::list_line_tests::attempts_blockers_and_notes_are_shown_when_there_are_any | crates/trellis-cli/src/work.rs:744:8: delete ! in list_line
killed (#248) by work::list_line_tests::ages_are_in_the_largest_whole_unit | crates/trellis-cli/src/work.rs:756:5: replace ago -> String with String::new()
killed (#248) by work::list_line_tests::ages_are_in_the_largest_whole_unit | crates/trellis-cli/src/work.rs:758:9: delete match arm 0..60 in ago
killed (#248) by work::list_line_tests::ages_are_in_the_largest_whole_unit | crates/trellis-cli/src/work.rs:756:5: replace ago -> String with "xyzzy".into()
killed (#248) by work::list_line_tests::ages_are_in_the_largest_whole_unit | crates/trellis-cli/src/work.rs:759:9: delete match arm 60..3600 in ago
killed (#248) by work::list_line_tests::ages_are_in_the_largest_whole_unit | crates/trellis-cli/src/work.rs:760:9: delete match arm 3600..86_400 in ago
killed (#248) by tests/orchestration.rs a_claim_records_where_it_was_made_under_release_and_only_there | crates/trellis-cli/src/work.rs:784:21: replace == with != in work
unexercisable on Windows (#248): needs a Ctrl-C delivered to the `work` process, which a test can send only on unix (`kill -INT`, as tests/interrupt.rs does); a unix-only `work --every` + SIGINT test is the kill, not written here because it could not be seen failing on this host | crates/trellis-cli/src/work.rs:861:5: replace listen_for_ctrl_c with ()
killed (#248) by tests/orchestration.rs an_approve_mode_agent_waits_when_work_has_no_terminal_to_ask_on | crates/trellis-cli/src/work.rs:937:9: replace CliRunner<'_>::approve_mode -> bool with false
killed (#248) by tests/orchestration.rs an_approve_mode_agent_waits_when_work_has_no_terminal_to_ask_on | crates/trellis-cli/src/work.rs:983:5: replace has_terminal -> bool with true
unexercisable by `cargo test` (#248): differs only when stdin and stdout are both a terminal, which needs a pty the suite does not have | crates/trellis-cli/src/work.rs:983:5: replace has_terminal -> bool with false
killed (#248) by work::runner_tests::a_tick_says_how_many_fires_it_dropped_and_says_nothing_when_none | crates/trellis-cli/src/work.rs:1024:33: replace > with == in <impl Runner for CliRunner<'_>>::tick
killed (#248) by work::runner_tests::a_tick_says_how_many_fires_it_dropped_and_says_nothing_when_none | crates/trellis-cli/src/work.rs:1024:33: replace > with < in <impl Runner for CliRunner<'_>>::tick
killed (#248) by work::runner_tests::a_tick_says_how_many_fires_it_dropped_and_says_nothing_when_none | crates/trellis-cli/src/work.rs:1024:33: replace > with >= in <impl Runner for CliRunner<'_>>::tick
killed (#248) by tests/orchestration.rs an_approve_mode_agent_waits_when_work_has_no_terminal_to_ask_on | crates/trellis-cli/src/work.rs:1087:37: delete ! in <impl Runner for CliRunner<'_>>::gate
killed (#248) by work::runner_tests::next_due_is_the_time_left_until_the_soonest_routine | crates/trellis-cli/src/work.rs:1110:9: replace <impl Runner for CliRunner<'_>>::next_due -> Option<Duration> with None
killed (#248) by work::runner_tests::next_due_is_the_time_left_until_the_soonest_routine | crates/trellis-cli/src/work.rs:1111:79: replace - with + in <impl Runner for CliRunner<'_>>::next_due
killed (#248) by work::runner_tests::next_due_is_the_time_left_until_the_soonest_routine | crates/trellis-cli/src/work.rs:1110:9: replace <impl Runner for CliRunner<'_>>::next_due -> Option<Duration> with Some(Default::default())
killed (#248) by work::runner_tests::next_due_is_the_time_left_until_the_soonest_routine | crates/trellis-cli/src/work.rs:1111:79: replace - with / in <impl Runner for CliRunner<'_>>::next_due
killed (#248) by work::runner_tests::the_runner_names_its_task_and_offers_a_process_probe | crates/trellis-cli/src/work.rs:1115:9: replace <impl Runner for CliRunner<'_>>::starting with ()
killed (#248) by work::runner_tests::the_runner_names_its_task_and_offers_a_process_probe | crates/trellis-cli/src/work.rs:1119:9: replace <impl Runner for CliRunner<'_>>::liveness -> Option<&dyn trellis_queue::Liveness> with None
killed (#248) by tests/queue.rs work_sends_no_key_to_an_endpoint_the_repository_chose (under the mutant the key reached the repository's port) | crates/trellis-cli/src/work.rs:1196:34: replace && with || in <impl Runner for CliRunner<'_>>::run
killed (#248) by work::runner_tests::idle_waits_out_its_time_unless_stopped_and_says_which | crates/trellis-cli/src/work.rs:1263:9: replace <impl Runner for CliRunner<'_>>::stop_requested -> bool with false
killed (#248) by work::runner_tests::idle_waits_out_its_time_unless_stopped_and_says_which | crates/trellis-cli/src/work.rs:1267:9: replace <impl Runner for CliRunner<'_>>::idle -> bool with true
killed (#248) by work::runner_tests::idle_waits_out_its_time_unless_stopped_and_says_which | crates/trellis-cli/src/work.rs:1267:9: replace <impl Runner for CliRunner<'_>>::idle -> bool with false
killed (#248) by work::runner_tests::idle_waits_out_its_time_unless_stopped_and_says_which | crates/trellis-cli/src/work.rs:1267:36: replace + with - in <impl Runner for CliRunner<'_>>::idle
killed (#248) by work::runner_tests::idle_waits_out_its_time_unless_stopped_and_says_which | crates/trellis-cli/src/work.rs:1268:30: replace < with == in <impl Runner for CliRunner<'_>>::idle
killed (#248) by work::runner_tests::idle_waits_out_its_time_unless_stopped_and_says_which | crates/trellis-cli/src/work.rs:1268:30: replace < with > in <impl Runner for CliRunner<'_>>::idle
equivalent (#248): at `now == until` the mutant runs one more pass that sleeps `min(100ms, 0)` and checks the flag again; the wait, the flag and the answer are the same | crates/trellis-cli/src/work.rs:1268:30: replace < with <= in <impl Runner for CliRunner<'_>>::idle
killed (#248) by work::runner_tests::idle_waits_out_its_time_unless_stopped_and_says_which | crates/trellis-cli/src/work.rs:1276:9: delete ! in <impl Runner for CliRunner<'_>>::idle
killed (#248) by context_files::installed_tests::a_user_s_own_skill_alone_is_reported_with_no_installed_count | crates/trellis-cli/src/context_files.rs:86:52: replace == with != in user_skills_note
killed (#248) by context_files::installed_tests::a_user_s_own_skill_alone_is_reported_with_no_installed_count | crates/trellis-cli/src/context_files.rs:96:26: replace && with || in user_skills_note
killed (#248) by context_files::installed_tests::a_user_s_own_skill_alone_is_reported_with_no_installed_count | crates/trellis-cli/src/context_files.rs:103:18: replace > with >= in user_skills_note
killed (#248) by context_files::tests::a_context_file_exactly_at_the_cap_loads_and_so_does_a_system_file | crates/trellis-cli/src/context_files.rs:325:30: replace > with >= in context_files
killed (#248) by context_files::tests::a_context_file_exactly_at_the_cap_loads_and_so_does_a_system_file | crates/trellis-cli/src/context_files.rs:378:19: replace > with >= in system_file
killed (#248) by context_files::tests::files_off_is_said_for_a_user_file_alone_and_not_when_there_is_none | crates/trellis-cli/src/context_files.rs:405:40: replace + with - in load
killed (#248) by context_files::tests::files_off_is_said_for_a_user_file_alone_and_not_when_there_is_none | crates/trellis-cli/src/context_files.rs:406:16: replace > with >= in load
killed (#248) by context_files::tests::a_skill_forced_twice_loads_once_and_each_named_skill_loads | crates/trellis-cli/src/context_files.rs:432:59: replace == with != in load
#248 | crates/trellis-cli/src/rpc.rs:104:9: replace <impl Visit for Fields>::record_i64 with ()
#248 | crates/trellis-cli/src/rpc.rs:107:9: replace <impl Visit for Fields>::record_bool with ()
#248 | crates/trellis-cli/src/rpc.rs:330:9: replace <impl trellis_loop::Model for Watched<'_, M>>::tokens_used -> u64 with 0
#248 | crates/trellis-cli/src/rpc.rs:330:9: replace <impl trellis_loop::Model for Watched<'_, M>>::tokens_used -> u64 with 1
#248 | crates/trellis-cli/src/rpc.rs:334:9: replace <impl trellis_loop::Model for Watched<'_, M>>::prompt_bytes -> Option<usize> with None
#248 | crates/trellis-cli/src/rpc.rs:334:9: replace <impl trellis_loop::Model for Watched<'_, M>>::prompt_bytes -> Option<usize> with Some(0)
#248 | crates/trellis-cli/src/rpc.rs:334:9: replace <impl trellis_loop::Model for Watched<'_, M>>::prompt_bytes -> Option<usize> with Some(1)
#248 | crates/trellis-cli/src/rpc.rs:338:9: replace <impl trellis_loop::Model for Watched<'_, M>>::compactions -> &[trellis_loop::Compacted] with Vec::leak(Vec::new())
#248 | crates/trellis-cli/src/rpc.rs:342:9: replace <impl trellis_loop::Model for Watched<'_, M>>::requested_compactions -> &[trellis_loop::RequestedCompaction] with Vec::leak(Vec::new())
killed (#248) by rpc::tests::in_flight_forgets_answered_waiters_and_keeps_the_ones_still_waiting | crates/trellis-cli/src/rpc.rs:611:33: delete ! in Server::in_flight
#248 | crates/trellis-cli/src/rpc.rs:613:24: replace match guard !r.thread.is_finished() with true in Server::in_flight
#248 | crates/trellis-cli/src/rpc.rs:799:21: replace match guard cmd.get("args").is_some() with false in Server::prompt
#248 | crates/trellis-cli/src/rpc.rs:802:21: replace match guard task.trim().is_empty() with false in Server::prompt
killed (#248) by rpc::tests::a_hosted_model_named_in_the_prompt_is_the_client_s_choice | crates/trellis-cli/src/rpc.rs:809:55: replace || with && in Server::prompt
#248 | crates/trellis-cli/src/rpc.rs:1088:5: replace summary -> Value with Default::default()
killed (#248) by tests/cli.rs session_bang_says_only_a_failing_exit_and_prompts_and_models_print_their_lists | crates/trellis-cli/src/session_extra.rs:30:18: replace match guard s.success() with true in shell
killed (#248) by tests/cli.rs session_bang_says_only_a_failing_exit_and_prompts_and_models_print_their_lists | crates/trellis-cli/src/session_extra.rs:30:18: replace match guard s.success() with false in shell
killed (#248) by tests/cli.rs session_bang_says_only_a_failing_exit_and_prompts_and_models_print_their_lists | crates/trellis-cli/src/session_extra.rs:68:5: replace print_prompts with ()
killed (#248) by tests/cli.rs session_bang_says_only_a_failing_exit_and_prompts_and_models_print_their_lists | crates/trellis-cli/src/session_extra.rs:82:12: delete ! in print_prompts
killed (#248) by tests/cli.rs session_bang_says_only_a_failing_exit_and_prompts_and_models_print_their_lists | crates/trellis-cli/src/session_extra.rs:124:5: replace print_models with ()
killed (#248) by session_extra::tests::the_reader_reads_nothing_after_end_of_input | crates/trellis-cli/src/session_extra.rs:296:54: replace || with && in Reader::spawn
killed (#248) by session_extra::tests::the_reader_reads_nothing_after_end_of_input | crates/trellis-cli/src/session_extra.rs:296:47: replace || with && in Reader::spawn
killed (#248) by session_extra::tests::a_stopped_reader_reads_no_line_after_the_one_in_flight | crates/trellis-cli/src/session_extra.rs:306:9: replace Reader::stop with ()
killed (#248) by session_extra::tests::late_lines_are_named_not_delivered_blank_ones_are_not_and_end_of_input_is_said | crates/trellis-cli/src/session_extra.rs:314:9: replace Reader::drain_late -> bool with true
killed (#248) by session_extra::tests::late_lines_are_named_not_delivered_blank_ones_are_not_and_end_of_input_is_said | crates/trellis-cli/src/session_extra.rs:314:9: replace Reader::drain_late -> bool with false
killed (#248) by session_extra::tests::late_lines_are_named_not_delivered_blank_ones_are_not_and_end_of_input_is_said | crates/trellis-cli/src/session_extra.rs:317:28: replace match guard !l.trim().is_empty() with true in Reader::drain_late
killed (#248) by session_extra::tests::late_lines_are_named_not_delivered_blank_ones_are_not_and_end_of_input_is_said | crates/trellis-cli/src/session_extra.rs:317:28: replace match guard !l.trim().is_empty() with false in Reader::drain_late
killed (#248) by session_extra::tests::late_lines_are_named_not_delivered_blank_ones_are_not_and_end_of_input_is_said | crates/trellis-cli/src/session_extra.rs:317:28: delete ! in Reader::drain_late
killed (#248) by session_extra::tests::the_line_in_flight_is_handed_to_the_session_as_typed | crates/trellis-cli/src/session_extra.rs:330:9: replace Reader::next_line -> Option<String> with None
killed (#248) by session_extra::tests::the_line_in_flight_is_handed_to_the_session_as_typed | crates/trellis-cli/src/session_extra.rs:330:9: replace Reader::next_line -> Option<String> with Some(String::new())
killed (#248) by session_extra::tests::the_line_in_flight_is_handed_to_the_session_as_typed | crates/trellis-cli/src/session_extra.rs:330:9: replace Reader::next_line -> Option<String> with Some("xyzzy".into())
killed (#248) by templates::tests::an_opening_fence_with_no_close_is_the_template_as_written | crates/trellis-cli/src/templates.rs:64:13: delete field body from struct Parsed expression in parse
killed (#248) by templates::tests::an_empty_description_or_hint_is_as_if_unset | crates/trellis-cli/src/templates.rs:85:30: replace match guard !v.is_empty() with true in parse
killed (#248) by templates::tests::an_empty_description_or_hint_is_as_if_unset | crates/trellis-cli/src/templates.rs:86:32: replace match guard !v.is_empty() with true in parse
killed (#248) by templates::tests::a_signed_index_in_braces_is_not_a_placeholder | crates/trellis-cli/src/templates.rs:357:24: replace && with || in braced
#248 | crates/trellis-cli/src/user_config.rs:64:5: replace skill_dirs -> Result<Vec<PathBuf>> with Ok(vec![])
#248 | crates/trellis-cli/src/user_config.rs:64:5: replace skill_dirs -> Result<Vec<PathBuf>> with Ok(vec![Default::default()])
#248 | crates/trellis-cli/src/user_config.rs:99:5: replace prompt_dirs -> Result<Vec<PathBuf>> with Ok(vec![])
#248 | crates/trellis-cli/src/user_config.rs:99:5: replace prompt_dirs -> Result<Vec<PathBuf>> with Ok(vec![Default::default()])
#248 | crates/trellis-cli/src/user_config.rs:110:5: replace installed_prompt_dirs -> Result<Vec<PathBuf>> with Ok(vec![])
#248 | crates/trellis-cli/src/user_config.rs:110:5: replace installed_prompt_dirs -> Result<Vec<PathBuf>> with Ok(vec![Default::default()])
#248 | crates/trellis-cli/src/user_config.rs:224:9: delete field agents from struct RepoConfig expression in layer
#248 | crates/trellis-cli/src/user_config.rs:225:9: delete field index from struct RepoConfig expression in layer
#248 | crates/trellis-cli/src/user_config.rs:234:33: replace match guard u == "list" with true in layer
#248 | crates/trellis-cli/src/user_config.rs:322:44: replace && with || in key_in::a_custom_tool
#248 | crates/trellis-cli/src/user_config.rs:400:23: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in RepoConfig::load_with_user
#248 | crates/trellis-cli/src/doctor_extra.rs:37:20: replace match guard f.is_file() with true in user_config_line
#248 | crates/trellis-cli/src/doctor_extra.rs:37:20: replace match guard f.is_file() with false in user_config_line
#248 | crates/trellis-cli/src/doctor_extra.rs:118:14: replace > with < in trust_line_with
#248 | crates/trellis-cli/src/doctor_extra.rs:132:17: replace > with < in trust_line_with
killed (#248) by doctor_extra::tests::a_live_service_runs_while_its_task_is_claimed_and_has_outlived_it_otherwise | crates/trellis-cli/src/doctor_extra.rs:208:13: replace && with || in services_line
killed (#248) by doctor_extra::tests::a_live_service_runs_while_its_task_is_claimed_and_has_outlived_it_otherwise | crates/trellis-cli/src/doctor_extra.rs:205:13: replace && with || in services_line
killed (#248) by doctor_extra::tests::a_live_service_runs_while_its_task_is_claimed_and_has_outlived_it_otherwise | crates/trellis-cli/src/doctor_extra.rs:204:9: delete ! in services_line
killed (#248) by doctor_extra::tests::a_live_service_runs_while_its_task_is_claimed_and_has_outlived_it_otherwise | crates/trellis-cli/src/doctor_extra.rs:207:65: replace || with && in services_line
killed (#248) by doctor_extra::tests::a_live_service_runs_while_its_task_is_claimed_and_has_outlived_it_otherwise | crates/trellis-cli/src/doctor_extra.rs:207:52: replace || with && in services_line
killed (#248) by doctor_extra::tests::a_live_service_runs_while_its_task_is_claimed_and_has_outlived_it_otherwise | crates/trellis-cli/src/doctor_extra.rs:207:70: replace == with != in services_line
killed (#248) by doctor_extra::tests::a_live_service_runs_while_its_task_is_claimed_and_has_outlived_it_otherwise | crates/trellis-cli/src/doctor_extra.rs:249:8: delete ! in services_line
```

</details>


## Tier 7 — Property and fuzz

Partial. Property tests exist (`partition_invariants`, `bidirectional`). The
parser is the obvious fuzz candidate and it is tree-sitter's, not ours — a
fuzzer there would test tree-sitter. The first-party candidate is
`UntrustedTrail::from_json`, which by construction parses
attacker-influenced input, and it now has a target (issue #95). The hosted client's
HTTP response parser, whose bytes a provider or proxy controls, has one
too (issue #97).

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

### `http_response`, at `91c270c`

`fuzz/fuzz_targets/http_response.rs` feeds raw bytes to
`trellis_model::fuzz_parse_response`, which passes them to
`transport::parse_response` (head parsing, `Content-Length`, `dechunk`) and
discards the result. A provider or a proxy on the path controls those bytes.
The function exists only under `cfg(fuzzing)`, which cargo-fuzz sets; the
ordinary build keeps `parse_response` crate-private (issue #97, ADR 0027).

Method: `scripts/checks/fuzz.sh http_response 300` at `91c270c`, empty
starting corpus, no dictionary, libFuzzer's default `-max_len` of 4096 bytes.
cargo-fuzz 0.13.2, rustc 1.100.0-nightly (1303417c4 2026-09-21),
AddressSanitizer, Windows x86_64, one process, seed 3725562845.

```
Done 1637261 runs in 301 second(s)
#1637261  DONE   cov: 424 ft: 1990 corp: 652/192Kb ... exec/s: 5439
```

**No crash and no timeout** (LeakSanitizer does not run on Windows). So there
is no crashing input to commit as a `fuzz_regression_http_response_<n>` test
in `trellis-model`.

What this does not establish: five minutes on one seed is a smoke run, not a
campaign. Inputs stop at 4096 bytes, so nothing near `MAX_RESPONSE` (32 MiB)
was tried; that cap is enforced by the reader in `post`, not by
`parse_response`, and the incremental `Framer` the reader uses to decide when a
response is whole is not exercised by this target.

## Tier 8 — Undefined behaviour

**N/A by construction.** `unsafe_code = "forbid"` at workspace level. No
`unsafe` block exists to audit, and one cannot be added without changing the
lint. Miri is therefore not informative here.

## Tier 9 — Concurrency

**Run, by audit rather than by tool**, because the result of the audit is that
there is very little for a tool to find.

**Superseded: this audit is of `94c285c` (written in `bdd792f`) and has not
been redone.** Since then, shared mutable state has been added (for example
`static JSON: AtomicBool` and `Arc<Mutex<dyn Write + Send>>` in
`trellis-cli/src/output.rs`), and `crates/tui-demo/src/bin/tui_demo.rs` has
a `#[tokio::main]`. The claims below, including the table's, hold at
`94c285c` only.

**There was no shared mutable state.** A workspace-wide search for `Mutex`,
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
| C-018 | 6 | `trellis-checks`, `trellis-core` and `trellis-model` survivors: the two mutants deciding Verified from Reviewed, a crate whose one test could not reach its own code, and the GiB figure users check their disk against | **Fixed** — 6 tests, all verified dead. `trellis-checks` and `trellis-model` were at zero survivors when this was written (`f0e86a9`); `trellis-model` has since grown (83 survivors at `989b2a2`, #245) |
| C-017 | 6 | `trellis-trail`'s remaining survivors: the calendar behind every trail filename was correct at three dates and unasserted at the rest, `run_id`'s filename fallback was untested, and the integrity verdict users see had no assertion | **Fixed** — 4 tests. Five calendar mutants verified dead, and `mp - 9` vs `mp / 9` is separated by a February date alone |
| C-016 | 6 | Trail tamper-evidence worked only because `schemars` transitively enables `serde_json/preserve_order`. Declared nowhere, tested by nothing, and dropping that dependency would make every trail read as `Altered` | **Fixed** — both sides now hash through `Value`, so they agree whatever `Map` is. Verified in a build with the feature and one without |
| C-015 | 6 | `type_head` — what an annotation says the receiver is — had six survivors. The suite tested its effects through fixtures whose annotations are all a bare class name, so unions, quoted forward references, the subscript rule and the character class were unexercised | **Fixed** — five tests, four verified against their mutants |
| C-014 | 6 | `discover` — what the graph counts as first-party — was reached by no test. The suite builds every index from in-memory sources, so the skip-list, the extension filter and the sort that makes ids stable were all unexercised | **Fixed** — five tests, three verified against their mutants |
| C-013 | 6 | Dead code, fourth instance: `RadiusReport::all_edges_certain` and `radius::note_unresolved`, five survivors between them, zero references | **Fixed** — deleted |
| C-011 | 6 | Four guards in the resolver's attribute path could be replaced with `true` and the suite stayed green. Each mutant **emits an unsound edge** — a `Resolved` naming one of several candidates, or an attribute resolved into a function's body | **Fixed** — four tests, each verified against its own mutant. v1.0 gate condition 2 is "zero unsound edges", and the suite did not defend it on this path |
| C-010 | 6 | `Worktree::create` puts a git worktree at `.trellis/work/<run>/partition-<n>`, inside the repository. A linked worktree there reads as untracked, so a run that partitions dirties the tree and **the next run refuses to start** (ADR 0009) | **Fixed** — `work/` added to the `.trellis/.gitignore` the index cache scaffolds. The same wall `index.bin` built once already, in a different file |
| C-008 | 6 | `Trail::factual_is_intact` was dead code, and its three surviving mutants said nothing about the tests | **Fixed** — deleted. The integrity check actually wired in is `UntrustedTrail::integrity`, which takes the raw JSON off disk rather than a parsed `Trail`, because a trail is read back as untrusted data (S10) |
| C-009 | 6 | Per-crate mutation runs test only that crate, so dependents' tests never run and every score understates | **Measured** — `Interner::deserialize -> Default` survives `-p trellis-core` (1 test) and is killed twice by the workspace suite. `--test-workspace true` re-runs at `bb616ca`, in `docs/bench/mutants-by-crate-ws/README.md`: `trellis-vcs` 59.3% → 92.6%, `trellis-core` 50.0% → 70.0%, `trellis-model` not computable (5 timeouts); `trellis-checks`, the fourth affordable crate, was never reached, because the job was killed for system memory and not restarted |
| C-004 | — | The C-003 fix changed the cache's serialised shape without bumping `FORMAT_VERSION`; the commit's claim about it was unverified and its stated mechanism wrong | **Fixed**, `ec2463c` — found by review, not by any tier |
| C-005 | 3 | Three cache fixtures still used fixed temp paths — the last of the collision class from `c526c83` | **Fixed**, `ec2463c` |
| C-006 | 3 | Two of the new CLI assertions were vacuous: one passed on `"no tests ran"` matching `"no "` while the tools were in fact installed and running; the other checked for two phrases `explain` never emits | **Fixed**, `ec2463c` — both now written against output that was read |
| C-003 | 9 | Index determinism under rayon is depended on by the cache and never asserted | **Fixed**, `0ba3b5f` — the graph was deterministic, its bytes were not; interner no longer serialises a derivable map |
| — | 3 | Test fixtures collided between concurrent processes | **Fixed**, `c526c83` |
| — | 0 | Toolchain unpinned | **Fixed**, `94c285c` |
| — | 10 | No dependency policy; `cargo deny` could not pass | **Fixed**, `bc705e1` |

## What this campaign does not establish

The tiers that ran are the tiers that can run on this machine. Specifically:

- No tier exercised a live model runtime, because that needs one at or
  above the capability floor and there isn't one. The transport parser is
  fuzzed (Tier 7) and the hosted client is tested against a stub. `adapter.rs`
  at 61.90% function coverage at `94c285c` (`docs/bench/coverage-20260921.md`)
  was that gap, measured.
- Tier 4 is not an independent review and does not claim to be.
- A mutation score is a statement about the tests, not about the code. A high
  score on a suite that asserts the wrong thing is worth nothing, and nothing
  here checks that the assertions are the right ones.
