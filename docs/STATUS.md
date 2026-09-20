# STATUS

Part 8 of the brief is the *project's* definition of done, not any one
session's. This maps each item to its actual state, so nothing is quietly
assumed complete.

Last updated 2026-09-20. Gate tables for v0.2, v0.3 and v1.0 are
below, each audited against the code rather than against the milestone prose.

| # | Part 8 item | State | Where |
|---|---|---|---|
| 1 | Every document in Part 6 exists | **Done** | `docs/`, 9 documents + 10 ADRs |
| 2 | `ASSUMPTIONS` block, every default chosen where the brief was silent | **Done** | `docs/ASSUMPTIONS.md`, 23 entries |
| 3 | All soft spots in `RISK_REGISTER.md` with a costed patch each | **Done** | `docs/RISK_REGISTER.md`, all 20 |
| 4 | **S1 resolved**: measured resolver reach, fallback trigger recorded | **Done for v0.1.** Hand-audited on two independent samples; 72.0% on the fresh one. A third audit is required before any v1.0 gate claim (A-34) | `trellis-index/src/resolve.rs`, `BENCHMARK_AND_EVAL.md` §2 |
| 5 | **S14 resolved**: named benchmark repo, threshold with reasoning | **Done** | `BENCHMARK_AND_EVAL.md` §1, §3 |
| 6 | **S16 resolved**: the recall figure that fires the pivot | **Done** — 60% | ADR 0002, `ROADMAP.md` |
| 7 | **S7 resolved**: model floor across seven metrics | **Partial.** Two configurations measured, both below the floor — two endpoints, no interval. Four of the seven metrics measured; §5.4 lists the other three with what each would take | `BENCHMARK_AND_EVAL.md` §5 |
| 8 | **S17 resolved**: llguidance embedded, inference path in an ADR, licence verified, matrix published | **Mostly.** Embedded and tested; licence verified MIT 1.8.0; matrix published. **Inference path deliberately open** | ADR 0006, `TECH_SPIKE.md` §3 |
| 9 | Grounding contract structural — `Verdict` unforgeable, `ToolResult` has no pass variant | **Done** | `trellis-checks/src/verdict.rs`, `trellis-loop/src/tools.rs` |
| 10 | One main loop with tools, not a verification fan-out, ADR says why | **Done.** Both halves demonstrated: a converging run, and a model declaring success while the checks disagree. The loop's *shape* is what this row is about; v0.2's full gate is not met — see the audit below | ADR 0005, `trellis-loop/src/agent.rs` |
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

## The v1.0 gate, read against today

`ROADMAP.md` fixes five conditions plus one, verbatim and without substitutions.
This is where each stands. Two are met, one is met with a scripted model and not
a real one, and three need measurement rather than code.

| # | v1.0 condition | State |
|---|---|---|
| 1 | Recall on the benchmark repo meets the v0.1 threshold | **No. 72.0%** against 85%, CI [63.2%, 80.8%]. In the S1 fallback band, which requires a further backstop. Both candidate backstops are measured and neither is worth building — see below |
| 2 | Zero unsound edges; every `Resolved` edge verified real | **Mechanically yes, 5,908/5,908**, and precision 100% on two hand audits. A third independent audit is required before the claim (A-34) |
| 3 | A real change to human approval, complete trail, grounded checks | **Partial.** Done end to end on poetry with a scripted model (`wedge-20260920.md`): defect, blast radius, failing check, edit, passing check, human `git commit`, trail, cold resume. Never with a model making the judgments |
| 4 | Induced mid-run failure returns the repo to its exact pre-run state | **Yes**, with a named `rollback-gate` CI job |
| 5 | Resumption works | **Yes**, demonstrated cold on the benchmark repo |
| + | Acceptable operation on a local open-weight model at or above the floor | **No, and the floor is not located.** Two configurations measured, both below it: 0.5B 0/20, Coder-3B 0/17 |

Conditions 3 and the plus-row are the same fact twice: no model has been found
that can drive this. That is a measurement result about available small models,
not a harness defect — the A/B in §5 is what separates those two, and it came
back saying the harness gave the model what it needed.

## Next, in order

*Rewritten wholesale each iteration rather than edited — editing is the
operation that let it go stale three times in one session.*

1. **Tier 3** (`AdapterTier::HarnessConstrained`). The one v0.2 gate item that
   is pure code, needs no network and no person, and the brief calls it "the
   one that makes any model work regardless of training". It is named in the
   enum and constructed nowhere; TRELLIS only ever speaks tier 2 over HTTP.
   The pieces exist — the probe rig already applies an llguidance mask in
   process — but that is the measurement rig, not the product.

   It is also the only route to the plus-row above that does not depend on
   finding a better model: tier 3 is what a runtime with no grammar support
   gets, and every local runtime a user actually has is on that path.

2. **Bonsai-2-27B served.** The default model is downloaded with TRELLIS and
   `trellis models` lists it, but nothing here can execute it: the candle probe
   uses `quantized_qwen2` and the weights are Qwen35 with ternary quantization.
   Serving it needs llama.cpp. Until then the default model is a catalog entry,
   which the catalog says plainly rather than implying otherwise.

3. **The human re-audit** (A-34, Q-15) — the only thing standing between the
   project and a call on gate condition 2.

   **The packet is ready to produce and the pipeline is validated.** Two
   commands, no network: `audit_sample ... 1000 <fresh-seed>` then
   `split_audit.py <dir> 40`, giving 25 blind chunks of 40. Round-tripped
   through split, merge and score with 1,000 synthetic judgements.

   It must report **wrongly excluded** as well as recall: ADR 0015 lets a site
   leave the denominator on the strength of an annotation, and no automated
   check can see a wrong exclusion (S21). If `resolve.rs` moves before scoring,
   regenerate `answers.json` — the seed reproduces the sites, not the verdicts.

4. **Conflict escalation** for non-negotiable rules, v0.3's one open gate item.
   Deliberately last: the honest version needs a model that can name the rule it
   could not satisfy, and building it against a fixture would test the fixture.

**v0.1 is complete. v0.3 is eight of nine. v0.2 is five of ten** — both gates
audited line by line below, against the code rather than against the milestone
prose that had them marked done. Every known latent unsound edge is closed and
the tool has an interactive session.

### Not on the critical path, but real

- **The session is a REPL, not a TUI.** It has line editing and per-repository
  history now (`rustyline`, default features off, seven transitive packages).
  Still no completion and no live status pane; a full-screen interface is a
  much larger piece and has not been costed.
- **`discover()` does not read `.gitignore`.** It skips a fixed list — `.venv`,
  `node_modules`, `site-packages`, `__pycache__`, `build`, `dist` — which
  covers the cases that matter in practice. A repository with a large ignored
  Python tree gets it indexed anyway; in this one that is `.bench/`, 914 files.
  Changing it would move the corpus and therefore every measurement, so it is
  recorded rather than done.

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

## v0.3's exit gate, audited against the code

Eight of nine. Each row cites what would have to fail for the row to be wrong,
because "v0.3 complete" was in this document twice on the strength of the
milestone prose rather than this checklist.

| Gate item | State |
|---|---|
| Every run writes a committed trail, factual and judgment split | **Yes.** Written after rollback, committed before the next run can start |
| The factual section carries provenance and is tamper-evident | **Yes.** `schema`, `run_id`, `base_commit`, `provenance` (TRELLIS version and model spec), and SHA-256 over the canonical JSON of `factual`. Editing a committed trail and leaving the digest makes `log` print `ALTERED` |
| Trail contents are untrusted on read-back | **Yes.** `UntrustedTrail`; `Verdict` has no `Deserialize`, so a recorded check reads back inert as `RecordedCheck` |
| Context, prompts and governance rules load from `.trellis/` | **Yes.** `context.md`, `prompts/<name>.md`, `governance/*.toml`. A malformed rule is an error, never a skip |
| A governance rule fails an audit **mechanically** | **Yes**, as of this iteration, and it was not before — see below. `CheckKind::Governance` produces a sealed `Verdict`; a broken non-negotiable fails the run and rolls it back, tested through the dispatch and not only at the audit function. Substring matching only — a governance check that exercised judgment would be Constraint 2 with extra steps |
| Rule ranking works; **conflicting non-negotiables escalate** | **Partial.** Ranking works and is tested. Conflict escalation is not built — below |
| `trellis resume` continues from durable state, not a replayed transcript | **Yes.** `--resume` seeds `PriorContext`, which is deliberately not `turns`, so a recorded pass cannot satisfy `last_check_passed` |
| `trellis why <symbol>` answers from the trail | **Yes**, after the wedge run found it could not |
| **The wedge, demonstrated** on the benchmark repo | **Yes.** `docs/bench/wedge-20260920.md` |

### The rule had no teeth on the path every model takes

Claiming the row above meant testing the wiring rather than the function under
it, and the wiring was broken.

`Orchestrator::run_checks` shadowed its `paths` argument:

```rust
let paths: Vec<PathBuf> = if paths.is_empty() {
    vec![PathBuf::from(".")]      // for mypy: "the whole repository"
} else { ... };
```

By the time the `Governance` arm ran, `paths.is_empty()` could never be true,
so the `self.touched` branch was dead code and the audit was handed `"."`.
`violations()` did `read_to_string(root.join("."))`, which fails on a
directory, and the failure went into a `let ... else { continue }`. The verdict
came back **passing**, saying `1 rule(s) checked over 1 file(s); none broken`,
having read nothing.

The severity is in which path that is. The floor runs found **0 of 10
`run_checks` calls passed a path** — a pathless call is what a real model
actually emits, every time, and it is the call `SCOPE:` was added to discourage
without success. So a non-negotiable rule could not fail in the only
circumstance it would ever be tested in, while `trellis check` listing the rule
and the unit test asserting it fails both read as working.

Two fixes, because there were two defects:

- The caller's paths are kept distinct from the defaulted ones, so the
  governance arm sees what was actually requested.
- **An audit reports coverage from what it read, never from what it was
  asked to read.** `Scan` now carries `read` and `unreadable`; a file a rule
  applies to and could not be opened is named as `NOT AUDITED`; and an audit
  given paths where *none* could be read **does not pass**, because passing
  there reports compliance that was never established, and `passed()` is what
  convergence reads. An empty request still passes — a run that touched
  nothing has broken nothing.

Both directions were checked by mutation rather than by the tests going green:
restoring the shadowed `paths` makes the scope test fail, and stubbing the
rules out of the dispatch makes the violation test fail. This is the third
instance this session of a guarantee documented and tested one layer below
where it was needed, after S20 and the always-empty `factual.checks`. The
pattern is stable enough to name: **test the call the model makes, not the
function it eventually reaches.**

### The one that is open

Two non-negotiable rules in conflict should escalate, and the harness should
not arbitrate between rules the user declared equally inviolable. The half that
needs no judgment exists: a broken non-negotiable fails the audit, the run does
not converge, the rollback is byte-exact.

What is missing is distinguishing a *conflict* from a *mistake* — establishing
that no fix satisfying rule A leaves rule B intact. Deciding that mechanically
needs the set of possible fixes, which the harness does not have. Deciding it
by asking the model is Constraint 2 returning through the back door, since the
model would be adjudicating the rules it is being held to.

The shape that would be honest: a non-negotiable failure where the model's own
summary names a second rule it could not satisfy, escalated verbatim and
attributed, with the harness declining to rule on it. That needs a model
capable of saying so, and both measured configurations are below the floor. Left
open rather than built against nothing and tested with a fixture that agrees.

### What running the wedge found

`trellis why` could not answer its own question. It reported "no file matching
`boolean_validator` was changed" about a run that had changed exactly the file
defining it. The factual section records **files**; a user asking why something
looks the way it does holds a **symbol**, and the two match only by coincidence
of naming.

That is the failure `03-v0.3-memory.md` names in its own "most likely to go
wrong" section — the trail written faithfully and useless because the grain is
wrong. No test caught it, because every test asserted the behaviour the code
had. It took running the thing the document says to run. Fixed by resolving the
symbol through the index to its defining file before matching, which works
retroactively: the mapping is recomputed, not recorded, so trails written before
the fix answer correctly.

## v0.2's exit gate, audited against the code

I had been calling v0.2 complete on the strength of the milestone documents in
`docs/`. Re-reading `02-v0.2-loop.md`'s own gate and checking each line against
the source says otherwise. Five met, three partial, two not — recorded here
because "complete" was the same defect as S20: a claim the code does not
support, in the document a reader consults to find out.

| Gate item | State |
|---|---|
| One real change on the **benchmark repo**, task to verified diff to approval | **No.** Demonstrated with a scripted model on a demo repository. Never on poetry, never with a real model — both measured configurations are below the floor |
| Induced mid-run failure rolls back, verified in CI | **Yes.** `rollback_after_an_induced_panic_returns_the_repo_to_its_prior_state`, plus a named `rollback-gate` job |
| `Verdict` unforgeable — **a compile-fail test proves it** | **Yes**, as of 2026-09-20. Was an assertion checked by reading the source; now three forgery attempts with committed compiler output |
| `ToolResult` has no pass variant | **Yes** |
| Two local models **plus one frontier API** through the adapter | **Partial.** Two local (0.5B, Coder-3B). No frontier API has driven the loop |
| **Tier 3 works**: a model with no native tool calling completes a task under harness-side constraint | **No.** `AdapterTier::HarnessConstrained` is named in the enum and constructed nowhere. TRELLIS only ever speaks tier 2 over HTTP |
| Budgets enforce; exceeding one escalates then rolls back | **Yes** |
| `trellis doctor` reports the active tier and what degrades | **Yes** |
| `Verified` and `Reviewed` tracked separately and shown at approval | **Partial.** The type forbids collapsing them (no `total()`), and every trail records `0/0`. No coverage mapping exists, so nothing ever populates them |
| Instrumentation: iterations, tokens, wall-clock, **context fill** | **Yes**, as of 2026-09-20. Context fill was not measured anywhere; each turn now records the prompt that produced it, in bytes, and the trail records first, peak and turns measured |

### The two that matter

**Tier 3 is the substantive gap.** The brief calls it "the one that makes any
model work regardless of training" and the default for local open weights. The
probe rig applies an llguidance mask in-process, which is exactly tier 3 — but
that is the *measurement rig*, not the product. TRELLIS speaks tier 2 to it
over HTTP. The floor results are correctly labelled tier 2; the gate item is
unmet.

**`Verified` / `Reviewed` is structurally right and empirically empty.** S3's
whole point is that "the callers still work" means two different things, and
the type makes them impossible to add together. It also makes them impossible
to be wrong about, because nothing measures them: deriving which tests cover
which call sites is unbuilt, so every run records `0 verified, 0 reviewed`. A
user reading that learns nothing, which is better than learning something
false, and worse than the gate asks for.

Neither blocks the v1.0 recall gate, which is what the remaining Next items
are about. Both block calling v0.2 done.

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
- **A run that repeated itself spent the whole budget finding out.** Typing
  "hello" into the session produced six identical `RunChecks { paths: [] }`,
  five minutes of CPU on a 0.5B, and a rollback. The same shape ran through
  both live runs and the entire 20-task floor. Three consecutive identical
  call-and-result pairs now stop the run; two are still allowed, because a
  check re-run after an edit that did not take is a legitimate second attempt.
  The **result** is compared, not just the call — re-running a check after a
  real edit is progress, and a stop keyed only on the call would kill the
  ordinary fix-then-verify loop.
- **A real model drove the loop end to end** (Qwen2.5-0.5B-Instruct Q4_K_M,
  tier 2, served locally). It reproduced the floor finding exactly: a pathless
  check, then five identical edits using the file path as the anchor and an
  invented filename as the replacement, every one refused for an absolute
  path, no convergence, byte-identical rollback. Constraint guarantees form,
  never judgement — demonstrated rather than tabulated.
- **`/model` probes the runtime** rather than only parsing the spec. A
  warning, never a refusal, and the spec is kept: you name a server then start
  it, and a tool that would not let you do that is wrong about the order
  people work in. Tier is reported at the same moment, because tier 4 is a
  materially worse run and a user should learn that before spending a budget.
- **A repository can name its own model** (`[model] spec` in config), so a
  project with a local runtime opens ready. An explicit `--model` still wins.
  Not a place for keys: ADR 0010 stands.
- **Pressing up-arrow in the session ran an agent task.** Without raw mode the
  key arrived as `ESC [ A`, which does not begin with `/`, so it fell past the
  command match into the model: slugified to "a", a trail written under that
  name, files edited. Control characters are refused in the read loop now, and
  `rustyline` makes the key do what it should. Reflex input must never start
  work.
- **History in `.trellis/history` would have broken the tool.** Untracked, so
  it dirtied the worktree — and a dirty worktree blocks the next run. A
  convenience feature re-erecting the wall `/accept` was added to knock down,
  two commits later. It lives in user state now, keyed by repository.
- **An interactive session** (`trellis` with no subcommand). One-shot was the
  wrong default: every invocation re-derived the index, answered one question
  and threw it away, while the orchestrator owns the index as a single-owner
  actor precisely so it can be held. The session holds the index, the model,
  and what the last run left pending.
  It closed a real wall: a converged run leaves its edits uncommitted on
  purpose, which blocks the next task, so /diff, /accept and /revert close
  that loop and the prompt shows the block before you walk into it. Eleven
  tests, the load-bearing one being that an unknown slash command is never run
  as a task.
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
- **v0.3's governance split.** `.trellis/context.md` informs the model;
  `deny_write` binds it, enforced in the orchestrator, which owns the only
  write path to the tree. A rule that lives only in a prompt is a suggestion.
  Rules gained teeth in the other direction too: a `forbid` pattern now fails
  an audit through `CheckKind::Governance`, producing a sealed `Verdict`,
  because a violation printed into the model's context and never entering a
  verdict is one a model converges straight past.
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

## The S7 floor: two configurations, both below it

**Qwen2.5-0.5B-Instruct Q4_K_M, candle on CPU, tier 2**, 20 verified tasks on
poetry 2.5.0. Full write-up and every deviation from the designed protocol in
`BENCHMARK_AND_EVAL.md` §5.

```
budget-exhausted 16   format-failure 3   no-recovery 1
0 repaired of 20      errors_after == errors_before on all 20
median 209 s per task
```

Nothing was repaired and nothing was made worse — the model never raised a
file's error count. Not a configuration that damages a repository; one that
does not move it.

### The second: Qwen2.5-Coder-3B-Instruct Q4_K_M, same protocol

Stopped by the host at task 17 of 20 for system memory pressure, unrelated to
the run, and reported at the size it reached.

```
0 repaired of 17      errors_after == errors_before on all 17
```

**The 3B reasons about the task and cannot address the repository.** It picks
the right file from the task text and writes a plausible edit — and emits the
path as `src/poeetry/...` in **all seventeen**, a transposition in the project
name, every time. Every such call is refused at the write path for a path
outside the tree, so the model spends its budget being told no in a way it
never recovers from.

I ruled out the harness before recording that: the tokenizer round-trips
`src/poetry/config/config.py` exactly, so the string the model produced is the
string it meant to produce.

**Two endpoints, no interval.** A floor is located by a configuration that
*passes*, and neither of these does. Nothing here licenses "the floor is above
3B" — a 3B tuned for tool use, or the same 3B under tier 3 where a grammar
could constrain the path against the real tree, are both untested. What the two
runs jointly establish is narrower and still worth having: at this scale the
failure is judgment, not format, and constraint of the kind TRELLIS currently
applies does not reach it.

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
