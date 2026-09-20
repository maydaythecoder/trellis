# STATUS

Part 8 of the brief is the *project's* definition of done, not any one
session's. This maps each item to its actual state, so nothing is quietly
assumed complete.

Last updated 2026-09-20.

| # | Part 8 item | State | Where |
|---|---|---|---|
| 1 | Every document in Part 6 exists | **Done** | `docs/`, 9 documents + 10 ADRs |
| 2 | `ASSUMPTIONS` block, every default chosen where the brief was silent | **Done** | `docs/ASSUMPTIONS.md`, 23 entries |
| 3 | All soft spots in `RISK_REGISTER.md` with a costed patch each | **Done** | `docs/RISK_REGISTER.md`, all 20 |
| 4 | **S1 resolved**: measured resolver reach, fallback trigger recorded | **Done for v0.1.** Hand-audited on two independent samples; 72.0% on the fresh one. A third audit is required before any v1.0 gate claim (A-34) | `trellis-index/src/resolve.rs`, `BENCHMARK_AND_EVAL.md` §2 |
| 5 | **S14 resolved**: named benchmark repo, threshold with reasoning | **Done** | `BENCHMARK_AND_EVAL.md` §1, §3 |
| 6 | **S16 resolved**: the recall figure that fires the pivot | **Done** — 60% | ADR 0002, `ROADMAP.md` |
| 7 | **S7 resolved**: model floor across seven metrics | **Partial.** One configuration measured, and it is below the floor — a lower bound, not a location. Four of the seven metrics measured; §5.4 lists the other three with what each would take | `BENCHMARK_AND_EVAL.md` §5 |
| 8 | **S17 resolved**: llguidance embedded, inference path in an ADR, licence verified, matrix published | **Mostly.** Embedded and tested; licence verified MIT 1.8.0; matrix published. **Inference path deliberately open** | ADR 0006, `TECH_SPIKE.md` §3 |
| 9 | Grounding contract structural — `Verdict` unforgeable, `ToolResult` has no pass variant | **Done** | `trellis-checks/src/verdict.rs`, `trellis-loop/src/tools.rs` |
| 10 | One main loop with tools, not a verification fan-out, ADR says why | **Done.** Both halves of the v0.2 exit condition demonstrated: a converging run, and a model declaring success while the checks disagree | ADR 0005, `trellis-loop/src/agent.rs` |
| 11 | Multi-agent parallelizes work only, ADR says why | **Done in design and rules; no concurrent execution yet** | ADR 0005, `trellis-loop/src/partition.rs` |
| 12 | Partitioning treats unresolved/candidate edges as unpartitionable; integrated checks never skipped | **Rule 1 implemented and property-tested over 400 seeds, now against a real resolver rather than synthetic graphs. Rule 2 documented; enforced when integration is built** | `partition::plan`, `tests/partition_invariants.rs` |
| 13 | `trellis doctor` reports batching capability and multi-agent tier | **Done.** Probes the runtime rather than reporting a default; verified reporting `RuntimeConstrained` against a live server | `trellis-model/src/tier.rs`, `trellis-model/src/runtime.rs` |
| 14 | Rollback verified by induced mid-run failure in CI | **Verified in the test suite. No CI configured yet** | `trellis-vcs/src/lib.rs` tests |
| 15 | Bidirectionality verified by property test | **Done** — 199 random graphs | `trellis-index/tests/bidirectional.rs` |
| 16 | v1.0 gate verbatim in `ROADMAP.md`, all five conditions | **Done** | `ROADMAP.md` |
| 17 | v1.0 non-goals explicit and substantial | **Done** — 10 entries | `ROADMAP.md` |
| 18 | Every major technology choice has a recorded rejected alternative | **Done** | ADRs 0001–0010 |
| 19 | Factual claims cited; unverifiable ones flagged | **Done** | `OPEN_QUESTIONS.md`, final section |
| 20 | No marketing language anywhere | **Done** | — |

## The measurement (resolves S1's reach; S16 fired, then averted)

Benchmark: poetry 2.5.0 sdist, SHA-256 in `01-v0.1-measure.md` §2.

| | first measurement | after ADR 0012 | after ADR 0015 |
|---|---|---|---|
| Recall, seed 20260919 (n=159) | **47.8%** | **67.9%** | **67.9%** |
| Recall, seed 20260920 (n=100, fresh) | — | **72.0%**, CI [63.2%, 80.8%] | **72.0%** |
| Precision | 100% | **100%** | **100%** |
| Wrongly excluded as external | 0 | 0 | **0** |
| Mechanical soundness check | 4,443/4,443 | 5,908/5,908 | **5,908/5,908** |

**ADR 0015 changed no recall figure, and that is the point.** It corrected the
resolver's own denominator — a receiver annotated `str` was being counted as a
first-party miss — which moved the repository-wide *bound* from 37.6% to 43.7%
while `resolved` stayed at 5,908. Hand-audited recall never depended on that
denominator, which is exactly why the gate is called on the audit. Both
completed audits were re-scored against the new resolver: 27 sites moved from
miss to exclusion on seed 20260920, and a human had independently judged every
one of them third-party.

47.8% fired the pre-registered S16 pivot trigger. The owner chose one bounded
backstop over the pivot (ADR 0011 option C, recorded as a deliberate deviation
in ADR 0012). Receiver typing cleared 60% on a **fresh** sample, confirmed
against the original, so **the pivot does not execute**.

**The S1 fallback band (60–85%) now applies.** The blast-radius guarantee
survives; a further backstop is required before v1.0 can claim it.

Three unsound edges appeared during the attempt and all three were fixed, each
at a cost in recall. The third was found only by the fresh sample: an
annotation that was simply wrong, with a *sibling* class overriding the method.
Annotations are evidence about the runtime type, never proof.

Findings that stand regardless:

- The deficit is in `Attribute` call sites. `Name` resolves at 84.7%.
- The brief expected `DecoratorIndirection` to dominate the misses. It is 3
  sites out of 11,233, against 10,646 for `MethodDispatchUntyped`.
- 51% of first-party misses cite pytest fixtures.

## The three that are genuinely open

Items 4, 7 and 10 still cannot be closed by writing documents. Item 4 now needs
only the audit scored. Items 7 and 10 need a working inference path and a
working agent loop, and both are v0.2.

## What was verified rather than assumed

- tree-sitter 0.27.0 pairs correctly with tree-sitter-python 0.25.0 at runtime
  (`tests/grammar_abi.rs`). The brief warns about this and it is now a test.
- llguidance 1.8.0 is MIT and actively maintained (upstream push 2026-09-04).
- `github/stack-graphs` is **archived**. Not an S1 option.
- `Kixiron/lasso` last shipped 2024-08-19. Replaced with a 40-line interner.
- `bincode` 3.0.0 is a protest release that deliberately fails to compile.
- `poetry.core` is a separate distribution, absent from the poetry tree, and
  imported 379 times. Treating it as first-party on the strength of its root
  package put those calls in the recall denominator as misses.

## Where the brief's own text needed correcting

- **llguidance API.** The brief's three-call sketch omits `CommitResult`'s
  `backtrack` and `ff_tokens`. Neither is optional: fast-forward tokens are a
  throughput win, backtracking is correctness. ADR 0006, `TECH_SPIKE.md` §3.
- **`Verdict` serialization.** A derived `Deserialize` would be a public
  constructor from arbitrary JSON, defeating the seal and opening the S10 hole.
  `Verdict` is `Serialize`-only; trails read back as inert `RecordedCheck`.
- **The dominant miss reason.** The brief predicts decorator indirection will
  dominate and it does not, by three orders of magnitude (above).

## Closed since the last update

`RadiusReport::coverage.unresolved` was structurally always zero: a reverse
walk cannot see an unresolved edge, because such an edge names no target and
never enters `reverse`. The radius now takes its unresolved count from the
files it touches — an over-approximation, which is the safe direction. Closes
OPEN_QUESTIONS Q-13.

## Next, in order

*Rewritten wholesale each iteration rather than edited — editing is the
operation that let it go stale three times in one session.*

1. **The model floor (S7), remainder.** One configuration measured and below
   the floor; the A/B says that is the model, not the harness. Locating the
   floor needs a configuration that *passes*, and Qwen2.5-Coder-3B is at
   931 MB of 2,105 MB, gaining roughly 56 KB/s — about six hours out. Blocked
   on bandwidth, not on work.

   `.bench/models/fetch.sh` resumes by HTTP range request and is safe to
   restart: it appends to the partial file rather than starting over, which is
   the only reason the download has survived thirty-seven attempts. If a
   session ends with it incomplete, restart it rather than re-fetching. When
   it lands, the floor run is one command —
   `python docs/bench/run_floor.py .bench/floor docs/bench/floor_tasks.json
   llamacpp:http://127.0.0.1:<port> <out>.json` against a probe server started
   on the new weights, and roughly 70 minutes.
2. **A human re-audit before any v1.0 gate claim** (A-34, Q-15). The only
   thing standing between the project and a gate call, and the only item that
   needs a person rather than a network.

   **The packet is ready to produce and the pipeline is validated.** Two
   commands, no network, documented in `docs/bench/README.md`:
   `audit_sample ... 1000 <fresh-seed>` then `split_audit.py <dir> 40`, giving
   25 blind chunks of 40. Round-tripped through split, merge and score with
   1,000 synthetic judgements, which caught the filled-chunk format before an
   auditor would have hit it rather than after.

   It must report **wrongly excluded** as well as recall: ADR 0015 lets a site
   leave the denominator on an annotation, and no automated check can see a
   wrong exclusion (S21). And if `resolve.rs` moves before scoring,
   `answers.json` must be regenerated — the seed reproduces the sites, not the
   verdicts.
3. **Cross-file type propagation, if the audit says the gate needs it.** The
   remaining `Attribute` bulk is attribute chains (1,969 sites) and locals
   whose type is not derivable within one file (1,883). Larger than anything
   attempted so far and not costed. Not to be started before the audit says
   how far short 85% actually is.
4. *(Nothing else. Q-14 is closed; see below.)*

**v0.1, v0.2 and v0.3 are complete** apart from S7's second configuration,
which is blocked on the network. What remains before v1.0 is the 85% recall
gate, and the next honest step there is measurement rather than code.

## The 85% backstop: both candidates measured, both too small, and a third found

The roadmap said the evidence pointed at `conftest.py` fixture return types and
return-type propagation. Both are now measured (`docs/bench/`), and neither is
worth building.

**Fixtures: already closed.** Parameter annotations have been collected as type
facts since ADR 0012 — `parse.rs` says so in a comment naming pytest fixtures
explicitly. poetry annotates its fixture parameters, so only **10** call sites
in the whole corpus have an unannotated test-function parameter as receiver,
and none of those names a fixture. `fixture_opportunity.py`.

**Return-type propagation: 67 sites.** One hop, both shapes — `f().m()` and
`x = f(); x.m()` — with the callee first-party, return-annotated, the return
type a first-party class, and the method present on it. Sixty-seven of 15,707,
which is 0.4 percentage points, against a rule that can only add unsound edges.
Not worth the risk.

**The third: the denominator is wrong, and by more than either rule would fix.**

`receiver_shapes.py` classifies every attribute-shaped call site by the shape
of its receiver. Two buckets stand out:

```
parameter, annotated non-first-party         1622   11.8%    val: str -> val.lower()
local, type not derivable in-file            1883   13.7%
local from call, return type not first-party  413
local from constructor call, non-first-party  230
local from annotation, non-first-party         92
```

`typed_receiver` ends with `let class = self.find_class(file, scope, &ty)?;`.
When the stated type is not a first-party class the `?` returns `None`, the
caller falls through, and the site is recorded `MethodDispatchUntyped` — an
**unresolved first-party miss**.

But `val: str` calling `val.lower()` is not a first-party call that was missed.
It is a call into the standard library, and `BENCHMARK_AND_EVAL.md` §2 defines
recall over call sites whose true callee is first-party. Such a site belongs in
`external`, excluded from the denominator, exactly like the 6,087 already
excluded there.

This is the same error that `poetry.core` produced: 379 imports counted as
first-party misses, and correcting it moved `Name` recall from 71.7% to 86.2%.
The shape is identical — the resolver knows enough to exclude a site and does
not.

Roughly 2,300 sites are in scope, which would move the denominator from 15,707
toward ~13,400. The reported figure would move accordingly. **That is not a
recall improvement and must never be presented as one** — no additional call
site becomes resolvable. It is a correction to a figure that has been wrong in
the pessimistic direction, and the only honest way to describe it is as a
denominator fix with the before and after both stated.

The discipline it needs, which is where it could go wrong: exclude only when
the type is *demonstrably* not first-party — a builtin, or a name that resolves
to an import from outside the root. An annotation the resolver merely fails to
resolve must stay a miss, or the rule becomes a way to launder every failure
into an exclusion and recall becomes unfalsifiable.

Next: implement that, re-run the audit on a fresh sample, and report both
denominators.

## What the last iteration found

**S20's patch was described in three documents and absent from the code.**
Commit `e99b375` says "Patched: `CheckConfig::default` now passes
`--no-incremental`". `RISK_REGISTER.md` says "Patch, applied." `Q-17` says
"The default now passes `--no-incremental`, which fixes mypy."
`crates/trellis-checks/src/lib.rs` was not in that commit, and the default was
a bare `vec!["mypy"]`.

So for several commits the repository asserted, in the document whose whole job
is an honest account of residual risk, that a hazard was closed while every
default run reopened it. S20 is the case where a `Verdict` passes on code the
checker never read; the register is exactly where someone would look to find
out whether it still can. Fixed, with the reasoning moved into the source and
two tests — one asserting the flag, one asserting TRELLIS does **not** append
it to a command the user configured, because a verdict about an invocation the
user never wrote is the same dishonesty inverted.

Worth generalising: a claim in a document is not evidence the code does it, and
this one survived a passing test suite, a clippy-clean build and a review of
its own commit message. Anything in `RISK_REGISTER.md` marked "applied" should
name the test that would fail if it were reverted.

## Recently closed

- **S6, which turned out not to be a legibility problem.** `trellis radius`
  printed "92 of 2474 first-party call sites resolved (3.7%)" — sites reaching
  the symbol over every unbindable site in the touched files. Two populations,
  one ratio, and the same string went to the model and into the trail.
  `RadiusCoverage` (ADR 0014) has no `total()` and no fraction. The per-file
  breakdown that replaced it showed at once that 1,575 of `Factory`'s 2,382
  unbound sites are in five test files.
- **v0.3 context seeding.** `PriorContext` is not `turns`, so a recorded pass
  from an earlier run cannot satisfy `last_check_passed` — the obvious
  implementation would have let a model converge on its first move against a
  tree that no longer exists.
- **A pathless `run_checks` now says what it checked.** Found by the floor
  run: the model called it with no paths, mypy chose the whole repository,
  poetry's pre-existing errors came back looking like the model's own, and it
  edited a file it saw in that output.
- **Q-14 closed** — the last known latent unsound edge. A class-scope
  assignment shadowing a method (`run = staticmethod(run)`) still produced a
  confident edge to the `def`. Fixed; `ShadowedAmbiguous` occurs zero times in
  poetry's 21,794 call sites, so resolved, denominator, both audited recall
  figures and precision are all unchanged.
- **An audit of the CLI surface** found three user-facing statements that were
  stale or wrong: `doctor` printed a concrete adapter tier with no `--model`
  while its own footer said tier was unknown; `partitions` said `resume` was
  unbuilt; the CLI module doc said `run` predated inference. All three fixed,
  and the ten rules CLAUDE.md says need an ADR were verified in code rather
  than by reading the document.
- **v0.3 is complete.** The context file and governance rules landed, and
  the split between them is the point: .trellis/context.md informs the
  model, [governance] deny_write binds it, enforced in the orchestrator,
  which owns the only write path to the tree. A rule that lives only in a
  prompt is a suggestion.
- **A resumed session was told everything except what the last run verified.**
  `context_lines` read only the always-empty `factual.checks`.
- **The converging path had no test**, and the suite proved it: replacing the
  `Done` branch's `last_check_passed()` with `false`, so no run can ever
  converge, made exactly one test fail — the new one. Every test in
  `trellis-loop` asserted a *refusal* to converge; the positive case was shown
  by hand and encoded nowhere.
- **An edit that changes nothing reported `applied: true`** — writing identical
  bytes back, telling the model it acted, and entering the rollback set and the
  trail's proposed list. Refused now.
- **The fourth recurrence of the `cargo fmt` string-continuation defect.** A
  backslash-continued literal gets its indentation folded into the string,
  leaving runs of twenty spaces mid-sentence in text the user and the model
  both read. Four live instances, all moved to `concat!`, and three tests now
  assert the rendered strings carry no double space.

## The S7 floor: one configuration, measured

**Qwen2.5-0.5B-Instruct Q4_K_M, candle on CPU, tier 2**, 20 verified tasks on
poetry 2.5.0. Full write-up and every deviation from the designed protocol in
`BENCHMARK_AND_EVAL.md` §5.

```
budget-exhausted 16   format-failure 3   no-recovery 1
0 repaired of 20      errors_after == errors_before on all 20
median 209 s per task
```

**This bounds the floor from below; it does not locate it.** A floor is an
interval, and one configuration measured below it is one endpoint. The second
(Qwen2.5-Coder-3B Q4_K_M) is still downloading. Nothing here licenses "the
floor is 3B" or any other figure.

Nothing was repaired and nothing was made worse — the model never raised a
file's error count. Not a configuration that damages a repository; one that
does not move it.

### A claim I put here and then falsified

This section previously said "Constraint removes the format failure
completely" and "the adapter's format retries have never been exhausted".
Three of the twenty tasks then failed on format under tier 2. I wrote a commit
message acknowledging it and did not edit this document for six commits — the
same defect as S20's, in the same file that carries the finding about it.

The corrected statement: **constraint guarantees prefix validity, not
termination.** Every *token* was schema-valid. Three *generations* were not
complete objects, because the model emitted an arbitrarily long JSON string
until `max_tokens` ran out, and no grammar can bound a string's length.
`whitespace_flexible: false` does not reach inside a string value.

### What holds regardless of the remaining configurations

- **Constraint eliminates the format-*shape* failure.** In one unconstrained
  probe on the demo repository, the same model emitted a fenced block
  containing `repo.edit` — an invented tool, wrapped in prose. Under tier 2,
  no generation across the 20 tasks was off-schema.
- **And guarantees nothing whatever about content.** The same model fills
  valid calls with `old: "<old value>"`, `old: "..."`, the file path echoed
  into all three fields, line numbers as edit anchors, and control characters
  as path separators. That is the distinction S7 exists to measure, now
  demonstrated rather than asserted.
- **Two harness defects the run surfaced**, both fixed and listed above: the
  pathless check and the no-op edit. Both flattered a weak model into looking
  like it was progressing.

### The A/B: the harness was not the constraint

The same configuration against the harness carrying both fixes. It is the only
thing separating "this model is below the floor" from "this harness withheld
what it needed". Stopped by the host at task 9 for system memory pressure,
which is unrelated to the run; reported at the size it reached.

```
8 of 9 trajectories byte-identical to run 1
0 repaired, either run
0 of 10 run_checks calls passed a path, after the SCOPE: line
1 read_file call in 81 turns
```

The last two lines carry it. `SCOPE:` states in plain words that no paths were
given, that the whole repository was checked, that pre-existing failures are
not the model's, and that `paths` should name what the task names — and the
task names its file in its first sentence. The model passed no path in any
call, and read a file once in 81 turns.

The single divergence, t04, diverged into more of the same: run 1 ended at
three iterations on a format failure, run 2 spent all nine producing further
`old == new` edits with control characters for path separators. The fix made
the failure longer, not better, which is what a legible failure looks like
when the thing failing is judgment.

So the table above reads as a model result rather than a harness artefact —
for *this* configuration, on 9 tasks. It says nothing about a stronger one.

Cost on this rig: median 209 s per task, 9 turns each. Per-turn time grows
through a run because the probe re-prefills the whole transcript every call —
a property of the probe, not of TRELLIS, stated that way wherever the figure
appears.

- Incremental update: single-file save **39.7 ms p50**, 3.1× a cold build,
  with a property test asserting an updated session equals a cold one.
- CI, including a named job for v1.0 gate condition 4.
- ADR 0013, settling the inference path S17 left open.
- Orchestrator actor and the agent loop, both tested without a model.
- `trellis run`, with **both halves of the v0.2 exit condition demonstrated**:
  a converging run, and a model that declares success while the checks
  disagree — rolled back to a byte-identical tree.
- `.trellis/config.toml` is now actually read. It was documented as the source
  of check commands while nothing loaded it, so a repository could configure
  its checks and be silently ignored.
- The model adapter, tiers 2 and 4, against llama.cpp, vLLM and SGLang.
- 20 verified model-floor tasks on the benchmark corpus.
- `trellis doctor` probes rather than reporting defaults (S17, S18).
- **v0.3 trail and resume**: written after rollback so failures are recorded,
  committed so the next run can start and no later rollback deletes them, and
  read back as `UntrustedTrail` with judgment fenced (S10, S11).
- **The S7 floor runner** (`docs/bench/run_floor.py`), and four ways it could
  have measured nothing: LF anchors against a CRLF corpus (all twenty tasks
  skip, reported as a clean run), resetting to HEAD while HEAD accumulates
  defects, scoring from the run's own verdict rather than re-checking, and
  running the checker from the wrong directory. Each produces a plausible
  report rather than an error.
