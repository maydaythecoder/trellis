# Quality audit ledger

**Status: Passes 0–7 complete.** One item is deferred and named below: the `main.rs` split, which waits on a `trellis-cli` mutation baseline that four memory kills have prevented. The
prerequisite was not met when this began; it is now met except for Tier 6,
which is running. See *Prerequisite* below.

| | |
|---|---|
| Repository root | `C:\Users\mayda\Documents\dev\trellis` |
| Risk profile | **application** — a CLI binary. The eight crates are internal to one workspace; none is published, so `pub` mostly means "visible to a sibling crate" rather than public API. This matters in Pass 5. |
| Correctness campaign | `94c285c`, see CORRECTNESS_LEDGER.md — Tiers 0–5 and 7–11 done, Tier 6 running |
| Mutation score baseline | **running** against `94c285c`; the audit's §9 rule is not yet enforceable |
| Coverage baseline | **82.74% lines** against `94c285c` (`docs/bench/coverage-20260921.md`) |

## Prerequisite: met except for Tier 6

The quality audit's §0 requires a finished correctness campaign, because §9
makes mutation score the check that catches refactor damage: *"a refactor that
makes code harder to test shows up here and nowhere else."*

| Tier | State |
|---|---|
| 0 — Baseline | **Now met.** `rust-toolchain.toml` pins 1.93.0; `CORRECTNESS_LEDGER.md` written |
| 1 — Compilation | Clean. `cargo check --all-targets --all-features` |
| 2 — Lints | Clean, including `clippy::correctness` and `clippy::suspicious` |
| 3 — Tests | Green. 328 passing, 0 ignored |
| 4 — Manual review | Partial |
| 5 — Coverage | **Run.** 82.74% lines. Half the gap is one file: `main.rs` at 39.09%, which has no test |
| 6 — Mutation | **Running.** ~1,253 mutants. Still the missing safety net until it lands |
| 7 — Property/fuzz | Partial. Property tests exist (`partition_invariants`, `bidirectional`); no fuzz targets |
| 8 — UB / unsafe | **N/A.** `unsafe_code = "forbid"` at workspace level |
| 9 — Concurrency | **Run, by audit.** No shared mutable state anywhere in `crates/*/src`; five sites checked individually |
| 10 — Dependencies | **Run and clean.** `cargo audit` and `cargo deny check` both pass; `deny.toml` states the policy |
| 11 — Formal | N/A |

**Consequence for this audit, revised.** The original position was that
Passes 3–7 all wait for the mutation baseline. That was too broad, and the
distinction worth drawing is what the compiler covers.

Passes 3, 4 and 5 are **type-level and compiler-verified**: named constructors
replacing bool parameters, and visibility narrowing. Neither can make code
harder to test — narrowing made the compiler find dead code, which is the
opposite — so the mutation score is not the instrument that would catch a
mistake in them. They are done.

**Pass 6 (duplication) is held**, and it is the one the rule was written for.
§3 warns that duplication is cheaper than the wrong abstraction, and an
extraction that buries logic inside a shared helper is exactly the damage a
mutation score detects and a green test suite does not. It waits.

**The `main.rs` split is also held.** It is a move rather than a
restructuring, but it is large, and it is the change the CLI integration
suite was written to protect — which makes measuring that protection before
relying on it the right order.

## A caveat about independence

Most of this code was written in the same sessions that are now auditing it.
An audit of one's own work grades its own homework: the things I would notice
are the things I already thought about, and the blind spots are shared. Two
findings below are specifically the kind a second reader finds and an author
does not — the `Partition::new(true)` call sites and the visibility question —
and both were found by tooling rather than by reading.

Read this ledger as a tool-assisted pass, not an independent review.

## Baseline

```
tests            328 passing, 0 failed, 0 ignored
clippy           0 warnings (--all-targets --all-features -D warnings)
library source   19,020 lines across 44 files
largest files    main.rs 2527 · agent.rs 1794 · resolve.rs 1630 · session.rs 1527
longest fns      build_from_facts 277 · cmd_run 261 · run_seeded 224 · cmd_init 202
                 (crude: fn-to-fn span, includes doc comments — a pointer, not a measurement)
mutation score   pending (running against 94c285c)
coverage         82.74% lines · 82.52% regions · 86.34% functions
```

## Findings

| ID | Pass | Class | Location | Issue | Action | Rationale | Commit |
|----|------|-------|----------|-------|--------|-----------|--------|
| Q-001 | 1 | B | `trellis-index/src/incremental.rs:84` | `update()` returns `bool` with no way to read what `true` means | Renamed `try_update` | `try_` is the Rust convention for an operation that may decline; the why-comment explaining *when* it declines is untouched | `2a61ad6` |
| Q-002 | 1 | B | `trellis-cli/src/main.rs:1101` | `rollback_or_explain` — "or" reads as alternatives | Renamed `rollback_and_report` | It always rolls back and always reports; explaining is what happens *when the rollback fails*, not instead of it | `2a61ad6` |
| Q-003 | 3 | A | `trellis-loop/src/partition.rs` | `Partition::new(true)` / `Partition::new(false)` at three call sites | **Fixed** | `shared()` and `parallel()`. The two states are not symmetric — one is the conservative bucket the staging exists to keep separate — and `true` said none of that | `297dbe8` |
| Q-004 | 4 | B | `trellis-loop/src/adapter.rs` | `RuntimeModel::new(client, constrained)`, called as `new(client, !unconstrained)` | **Fixed** | `constrained()` and `unconstrained()`, matching the flag the user types. Not an `AdapterTier` parameter: that accepts tier 1 and tier 3, which this type cannot drive | `297dbe8` |
| Q-005 | 5 | B | workspace-wide | ~250 undocumented `pub` items | **Fixed** | `unreachable_pub` on at workspace level (17 items in the binary); 19 `pub mod` narrowed to `mod`, leaving the six module paths that cross a crate boundary. The re-exports already covered everything, and the compiler confirms nothing took the second path | `988f3f3` |
| Q-009 | 5 | B | `trellis-loop/src/tools.rs` | `pub mod args` — five argument structs used by nothing | **Fixed** | A design that was possible and never wired. `ToolCallWire`, one tagged enum compiled to one grammar, superseded per-tool argument structs, and `args` went dead when `wire.rs` landed — the production schema is `ToolCallWire::schema()` at `adapter.rs:265`. Narrowing the module surface is what exposed it | `988f3f3` |
| Q-006 | 2 | C | `trellis-checks/src/lib.rs:222` | `TODO(v0.2): parse structured output per tool` — v0.2 has passed | **Left alone** | It carries a milestone reference and the work is still genuinely open (`failure_count` is still `None`). Deleting it would lose a real open item; §4 only condemns a TODO with no owner, ticket, or date |
| Q-007 | 2 | B | `CLAUDE.md:32` | Claimed `run`, `resume` and `partitions` "exit with a clear 'not implemented yet'". All three have been implemented since v0.2 | **Fixed** | A stale instruction file is worse than none: a reader trusts it and stubs a command that already works. Rewrote it to keep the rule (do not fake a command) while stating the current fact | pending |
| Q-008 | 2 | — | workspace-wide | Suspected `cargo fmt` string-folding defects — prose literals broken mid-sentence by a source line break | **None found** | 198 literals span a line break; 17 survived filtering for mid-sentence prose; all 17 rendered and every one is a deliberate hand-wrap at 73–78 columns. The remainder are `\` continuations or literal test fixtures. Recorded so it is not re-litigated | — |

## Pass 2 result: almost nothing to delete, and one stale claim

The survey for §4's delete-list found no commented-out code, no section
banners, no changelog comments, and one TODO that carries a reference and is
still true. The two `grep` hits for commented-out code were prose lines that
happened to begin with `for`.

The additive half of Pass 2 — missing doc comments — collapsed into Q-005,
because the right fix for most of those items is narrowing visibility rather
than documenting them.

What the survey *did* find is Q-007: `CLAUDE.md` still told a reader that three
implemented commands were stubs. Comments were clean; the instruction file was
not, which is the inversion worth noticing — the file nobody compiles is the
one that rots.

Q-008 is a negative result kept on purpose. A prose string broken mid-sentence
by `cargo fmt` had been a real recurring defect in this repository, so the
whole workspace was rescanned. Seventeen candidates survived filtering and all
seventeen, when rendered, were deliberate hand-wraps. The scan is worth
repeating after a `cargo fmt` that touches output strings; it is not worth
repeating otherwise.

## Left alone deliberately

The section §11 says a reviewer should read first.

**Bool-returning predicates without an `is_`/`has_` prefix** — `in_scope`,
`passed`, `converged`, `all_edges_certain`, `coverage_available`,
`type_is_demonstrably_external`, `last_check_passed`. All read correctly at
the call site (`if !in_scope(rule, file)`, `v.passed()`). Class C. Renaming
them to satisfy a pattern would be churn.

**`focus_style(focused: bool)` and `panel(title, focused: bool)`** in the TUI.
Bool parameters, and called as `panel("transcript", app.focus == Focus::Log)`
— the expression at the call site carries the meaning. Class C. An enum here
would be ceremony for two private functions in one file.

**The long functions.** `build_from_facts` (277), `cmd_run` (261),
`run_seeded` (224). Each is one sequence of steps with no branching to hide,
and each is heavily commented with *why*. Splitting them would produce named
fragments with no independent meaning — the §8 trap where function count
improves and readability does not. Class C unless a specific reader gets lost;
no evidence of that yet.

**`main.rs` at 2,527 lines.** Genuinely large, and a candidate for splitting
by command. Held until Pass 5, where module structure is the subject and the
split can be made once rather than twice.

**Duplication.** Not surveyed yet. Pass 6 is last by design and its §3
warning — *duplication is cheaper than the wrong abstraction* — is the part of
this audit most likely to do harm. It waits for the mutation baseline, which
is precisely the signal that detects an extraction having buried logic.

## Pass 6 — surveyed, and it found a defect rather than an abstraction

The pass §3 warns about most: *duplication is cheaper than the wrong
abstraction*. Surveyed with a normalised-line scan for repeated runs of eight
or more statements, comments and formatting excluded.

**Every repeated block is in test or example code. Production code has none.**

Four clusters:

| what | copies | verdict |
|---|---|---|
| A git-repo test fixture | 7 | **Drifted — see below** |
| splitmix64, in the measurement examples | 3 | Left alone |
| xorshift64star, in the property tests | 2 | Left alone |
| A `sources()` fixture, cache.rs and incremental.rs | 2 | Left alone |

### The git fixture had already diverged

Seven copies set up a temporary git repository. Five pin
`core.autocrlf false`; **two did not** — `init_tests::repo` in `main.rs`, and
`repo()` in the CLI integration suite written earlier in this same audit.

That is not hypothetical harm. Git on Windows normalises line endings on
`add` without it, and `a_missing_scripted_model_fails_before_anything_is_touched`
asserts an empty `git status --porcelain` over files the test wrote itself.
It passed, but on the strength of nothing.

**Both fixed.** The extraction is not done: it could unify five of the seven —
`main.rs` and `session.rs` share a crate — while `tests/cli.rs` is a separate
crate and `trellis-vcs` another, so two copies survive any reasonable version
of it. A fix that reaches five sevenths of a class is worth doing with the
mutation score in hand rather than without it.

### The two generators, verified rather than assumed

`docs/bench/decomposition-20260921.md` claims *"the same splitmix64 generator
as the audit sampler, so a seed means the same thing across the project's
measurement tools."* Checked: the three measurement examples are
byte-identical splitmix64 (`0x9E3779B97F4A7C15`, `0xBF58476D1CE4E5B9`), and
the two property tests use xorshift64star (`0x2545F4914F6CDD1D`) — a different
generator, which the claim does not cover because it says *measurement tools*.

**The claim is true and correctly scoped.** Recorded because verifying it was
the point of looking, and a claim checked and found sound is worth as much
here as one found wrong.

Left alone deliberately. Six lines of a named standard algorithm, in three
standalone tools each meant to be read in isolation. The alternative is an RNG
in `trellis-core`, whose charter is ids, the interner and errors — stretching
a foundation crate to hold a measurement utility is the wrong abstraction §3
is warning about.

## A correction to Q-009

The row above was first recorded as Class A with a different reason: that the
deleted comment "described a mechanism the crate graph forbids", because
`trellis-model` does not depend on `trellis-loop`.

**That is wrong, and the commit message `988f3f3` carries the same error.**
The comment said trellis-model compiles the structs into a grammar, which
means `trellis_model::ToolGrammar::for_args::<T>()` — generic over any
`JsonSchema` type precisely so a caller can hand it one of its own. trellis-loop
depends on trellis-model, so that call was always legal. Nothing forbade it.

The real finding is smaller and the deletion still right: the structs were an
unwired design, superseded by `ToolCallWire` before anything called them.
Downgraded from A to B accordingly.

Recorded rather than quietly amended, the same way C-004 is in
CORRECTNESS_LEDGER.md. An audit that corrects its own record silently is
worth no more than the claims it was auditing.

## Pass 7 — lints as advisory

`cargo clippy -W clippy::pedantic -W clippy::nursery`, mined for readability
signal only. The pass says outright not to apply these mechanically, and most
pedantic hits are Class C.

**Seven of the eight lints it names as relevant fire zero times.**

```
too_many_arguments             0        wildcard_imports               0
cognitive_complexity           0        module_name_repetitions        0
fn_params_excessive_bools      0        missing_docs_in_private_items  0
type_complexity                0        doc_markdown                  15
```

`fn_params_excessive_bools` at zero is worth noting next to Q-003 and Q-004:
the defect there was never *many* bools, it was one bool whose meaning lived
in the callee. A lint would not have found either.

**`doc_markdown`, 15 hits, 14 false.** Clippy reads `OPEN_QUESTIONS.md`,
`CORRECTNESS_LEDGER.md`, `PyCG`, `Pyright`, `vLLM`, `SGLang` and `llama.cpp`
as Rust identifiers wanting backticks. They are document names, product names
and a command line. One was real and is fixed: `ModeCounts::summary`'s comment
said "There is no total() here on purpose" — `total()` genuinely is an item,
and the comment is load-bearing, so it now reads `` `total()` `` and cites
ADR 0003.

**`too_many_lines`, 6 hits, and a correction to this ledger's own baseline.**
Clippy counts statements rather than the fn-to-fn span this file measured
crudely at the start, and the difference is large:

```
                      crude (this ledger)   clippy   file
build_from_facts              277            234     resolve.rs:489
cmd_run                       261            188     main.rs:727
run_seeded                    224            171     agent.rs:351
```

The baseline said its own measure was "a pointer, not a measurement" and it
was right to: it over-counted by 25–40% because it included doc comments,
and these functions are heavily commented. They are still long. The decision
recorded under *Left alone deliberately* stands — each is one sequence of
steps with no branching to hide — but the numbers it was made against were
inflated and are corrected here.

**Not mined, and deliberately.** `cast_possible_truncation` is the largest
pedantic category at 47. Pass 7 is a readability pass and casts are not a
readability signal, so they are out of scope here. They are a *correctness*
question — a `usize as u32` that can truncate is a real defect class — and
belong to the campaign rather than the audit, and are recorded there as
C-007: examined, and clear.

## Escalations

**None requiring a decision yet.** Q-003 and Q-004 change private call-site
ergonomics only; neither touches public API. Q-005 would narrow visibility,
which is source-compatible within the workspace and verified by the compiler,
but is a large enough diff to want its own pass.

## Final metrics

```
                      at the start (94c285c)        now
tests                 328 passing                   415 passing
clippy                0 warnings                    0 warnings, plus unreachable_pub
library source        19,020 lines / 44 files       20,434 lines / 45 files
mutation score        none — did not exist          7 of 8 crates measured
coverage              none — did not exist          82.74% lines (94c285c)
```

**The line count went up, and that is the honest shape of this audit.** It
deleted four pieces of dead code and unified six fixtures into one, and it
added far more in tests — 87 of them — and in comments explaining why a guard
is written the way it is. An audit that shrank the tree would have been
optimising the metric §8 names as a trap.

### Long functions, corrected

Pass 7 replaced this ledger's own figures. The baseline measured a fn-to-fn
span, which includes doc comments and over-counted by 25–40% in a file this
heavily commented:

```
                  this ledger's baseline   clippy   file
build_from_facts             277            234     resolve.rs
cmd_run                      261            188     main.rs
run_seeded                   224            171     agent.rs
```

Still long; the decision under *Left alone deliberately* stands, and the
numbers behind it are now measured rather than estimated.

### What each pass produced

| Pass | Result |
|---|---|
| 1 — Naming | Two renames, Q-001 and Q-002 |
| 2 — Deletions | Almost nothing to delete, and one stale claim in `CLAUDE.md` (Q-007). A workspace rescan for the `cargo fmt` string-folding defect found **none** (Q-008) |
| 3 — Call sites | `Partition::{shared,parallel}` (Q-003). Complete: every other bool parameter is passed a named variable or a meaningful expression |
| 4 — Types | `RuntimeModel::{constrained,unconstrained}` (Q-004) |
| 5 — Visibility | `unreachable_pub` on; nineteen `pub mod` narrowed. Narrowing then exposed Q-009, dead code Pass 2 had walked past |
| 6 — Duplication | Production code has **none**. Every repeated block is in tests, and one class had already drifted — two of seven git fixtures had lost `core.autocrlf false`. Six unified into `testutil.rs` |
| 7 — Lints | **Seven of the eight readability lints fire zero times.** `doc_markdown` fires 15 times and 14 are clippy reading document and product names as identifiers |

### Deferred, with what it waits on

**The `main.rs` split.** 2,527 lines, and the obvious candidate for splitting
by command. It is the change the CLI integration suite was written to protect,
and §9 wants a mutation score to compare against — `trellis-cli` is the one
crate that has never completed a per-crate run, stopped four times by system
memory. Deferring it is the §7 call: the split is correct-and-large, the
measurement that would make it safe is not available, and a 2,500-line file
that works beats a half-split one that nobody measured.
