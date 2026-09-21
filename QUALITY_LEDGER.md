# Quality audit ledger

**Status: in progress.** Passes 0–2 complete, 3–7 not started. The
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

**Consequence for this audit.** Passes 1–2 are compiler-verified renames and
comment work, which the test suite alone is sufficient to check. Passes 3–7 —
particularly Pass 4 (types) and Pass 6 (duplication) — are where a refactor can
quietly make code less testable, and those are not started until a mutation
baseline exists.

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
| Q-003 | 3 | A | `trellis-loop/src/partition.rs:186,220,228` | `Partition::new(true)` / `Partition::new(false)` at three call sites | **Reported, not fixed** | §7 names this exactly. Fix is two named constructors (`shared()` / `parallel()`), which is Pass 3 and waits for the mutation baseline | — |
| Q-004 | 3 | B | `trellis-loop/src/adapter.rs:46` | `RuntimeModel::new(client, constrained: bool)`, called as `new(client, !unconstrained)` | **Reported, not fixed** | A double negative at the call site. An enum would read, but that is Pass 4 | — |
| Q-005 | 5 | B | workspace-wide | ~250 undocumented `pub` items | **Reported, not fixed** | Verified `trellis_loop::tools` and `trellis_loop::budget` are referenced from no other crate. These are over-wide visibility, not public API — the fix is narrowing, not writing 250 doc comments. Pass 5 |
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

## Escalations

**None requiring a decision yet.** Q-003 and Q-004 change private call-site
ergonomics only; neither touches public API. Q-005 would narrow visibility,
which is source-compatible within the workspace and verified by the compiler,
but is a large enough diff to want its own pass.

## Final metrics

Pending. Against a baseline of 328 tests and 0 clippy warnings, both are
unchanged after Pass 1. Mutation score and coverage cannot be compared until
the baseline exists.
