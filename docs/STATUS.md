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
| 1 | Recall on the benchmark repo meets the v0.1 threshold | **No. 72.0%** against 85%, CI [63.2%, 80.8%]. In the S1 fallback band, which requires a further backstop. Both candidate backstops are measured and neither is worth building — see below. **Now tracked rather than remembered**: the `precision-gate` CI job regenerates the resolver's verdicts and re-scores both seeds against the committed human judgements, failing below the recorded figures |
| 2 | Zero unsound edges; every `Resolved` edge verified real | **All three mechanical checks now pass over the full corpus**, not a sample: name match 5,908/5,908, and **import reachability 0 disconnected, 0 ambiguous** — the check that reaches "the wrong `foo` in another module", which the name check explicitly cannot (`docs/bench/precision-20260920.md`). Precision 100% on two hand audits. A third independent audit is still required before the claim (A-34) |
| 3 | A real change to human approval, complete trail, grounded checks | **Partial.** Done end to end on poetry with a scripted model (`wedge-20260920.md`): defect, blast radius, failing check, edit, passing check, human `git commit`, trail, cold resume. Never with a model making the judgments |
| 4 | Induced mid-run failure returns the repo to its exact pre-run state | **Yes, at seven injection points**, up from one — including a real SIGINT delivered mid-run and a dead orchestrator, the two `04-v1.0-harness.md` calls usually missed. Recovery-path failure handled: a rollback that cannot finish is loud, names the worktree, gives the commands, and no longer swallows the trail. `docs/bench/rollback-20260920.md` |
| 5 | Resumption works | **Yes**, demonstrated cold on the benchmark repo |
| + | Acceptable operation on a local open-weight model at or above the floor | **No, and the floor is not located.** Two configurations measured, both below it: 0.5B 0/20, Coder-3B 0/17 |

Conditions 3 and the plus-row are the same fact twice: no model has been found
that can drive this. That is a measurement result about available small models,
not a harness defect — the A/B in §5 is what separates those two, and it came
back saying the harness gave the model what it needed.

## Next, in order

*Rewritten wholesale each iteration rather than edited — editing is the
operation that let it go stale three times in one session.*

**Now the v1.0 task order from `04-v1.0-harness.md`**, which supersedes the
list this section carried through v0.3. Item 1 is done; tier 3 is carried
forward from v0.2 because the v1.0 list does not mention it and the plus-row
of the gate needs it.

1. ~~Full precision audit; fix every unsound edge found.~~ **Done** —
   `docs/bench/precision-20260920.md`. No unsound edge found, so nothing to
   fix. The new check is `examples/check_reachability.rs` and it runs in CI.
2. ~~Rollback hardening across all injection points.~~ **Done** —
   `docs/bench/rollback-20260920.md`. Seven, up from one. SIGINT needed code
   and a real signal to test; the orchestrator case needed code to *notice*.
3. ~~Recovery-path failure handling.~~ **Done**, with the same artifact. A
   rollback that cannot finish is loud, names the worktree, gives the exact
   commands, attempts both steps rather than stopping at the first, and no
   longer swallows the trail.
4. ~~Degradation UX (S6).~~ **Done** — `docs/bench/degradation-ux-20260920.md`.
   The approval moment had nothing; it now carries a scoped one-line coverage
   note and three paths, and never blocks.
5. ~~`trellis doctor` completion.~~ **Done.** All eight lines the milestone
   mockup asks for, each with a remedy — and two of them corrected where the
   mockup would have had TRELLIS claim something it cannot know.
6. **Re-run the v0.1 measurement for condition 1.** Largely absorbed into the
   `precision-gate` CI job, which now does it on every push.
7. ~~`trellis init`, first-run prerequisite detection, and a good error for
   the clean-worktree requirement.~~ **Done.** The scaffold changes nothing by
   default and never overwrites; the wall now says why it exists and gives
   three ways through it.
8. ~~Packaging and cross-compilation.~~ **Done.** Release profile tuned,
   a three-platform matrix that tests on the profile it ships, and the
   supported list stated as what was tested. It found the `Verdict`
   compile-fail proof pinned to one toolchain.
9. ~~Documentation: partial coverage, and `Verified` versus `Reviewed`.~~
   **Done.** The README was rewritten — it had said "pre-v0.1, the resolver
   and the agent loop do not [exist]" — and the audit-mode limit is now stated
   at the approval prompt, not only in a document.
10. **`MISSION.md` with honest positioning (S15)** — concede the index is
    commodity and argue the wedge where it actually is.
11. **Tier 3** (`AdapterTier::HarnessConstrained`), carried from v0.2. The
    gate's plus-row needs it and the doc's own `doctor` mockup prints
    `Tier 3 ✓`. Named in the enum, constructed nowhere.
12. **Gate proof**: all five conditions, artifacts recorded.

Two items remain that no amount of code closes: **the human re-audit** (A-34,
Q-15), whose packet is ready and pipeline validated, and **a model at or above
the floor**, which needs a configuration that passes rather than a third
endpoint below it.

**v0.1 is complete. v0.3 is eight of nine. v0.2 is five of ten** — both gates
audited line by line below, against the code rather than against the milestone
prose that had them marked done. Every known latent unsound edge is closed and
the tool has an interactive session.

### Not on the critical path, but real

- **There is now a TUI (`trellis tui`), and it contradicts a stated v1.0
  non-goal.** `04-v1.0-harness.md` lists "any surface beyond the CLI — no TUI"
  as out of scope, and this was built at the owner's explicit instruction
  after that document was written. Recorded as a deviation rather than
  quietly resolved: the roadmap says one thing and the repository now does
  another, and a reader should not have to discover that by running it.

  The line-based session remains the default; `trellis tui` is opt-in.
  Model-view-controller split across `tui/{model,view,events,terminal}.rs`,
  with the model holding no terminal and no I/O so the interface's behaviour
  is tested without one. The UI thread blocks on a single channel and never
  on work: runs go to a worker thread and report progress through `tracing`,
  which CLAUDE.md already designates the one telemetry path.

  Still no completion, and the transcript is the only pane that scrolls.
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

## `trellis doctor`: complete, minus two claims the mockup asks for

v1.0 task item 5. `doctor` reported model, runtime, tier, batching and two
yes/no tool lines. It now reports all eight lines `04-v1.0-harness.md` asks
for, and every line that can be wrong says what to do about it — a diagnostic
that reports a problem without a next step has only told the user they are
stuck.

Added: **floor**, **index**, **worktree**, **budget**; the tool lines carry
versions (`mypy 2.3.1`, `pytest 9.1.1`) rather than "found", because "found"
cannot distinguish the checker the user configured from a different one
earlier on `PATH`, and a verdict is only as good as the process that produced
it. The tests line also reports whether a coverage plugin exists, since
without one nothing can ever populate `Verified` and a user reading
`0 verified` should know whether that is a result or a missing tool (S3).

**Two lines deliberately do not match the mockup, and both would have been
this project's own documented failure:**

- The mockup prints `Index  48,102 symbols · 91.3% resolved ✓`. CLAUDE.md
  forbids exactly that — what the resolver *bound* is a lower bound on recall,
  never recall, and printing it beside a tick is how that claim finally
  escapes. This is the failure ADR 0014 already fixed once in `radius`, which
  printed "3.7% resolved" for a resolver hand-audited at 72%. The line states
  the bound, says **NOT recall** in the same breath, and names the hand audit
  as the only comparable figure.
- The mockup prints `Floor  above measured minimum ✓`. **TRELLIS cannot know
  that.** A floor is an interval located by a configuration that *passes*; two
  have been measured and both failed every task, so there is no minimum to be
  above. The line says the floor is not located and points at the evidence.

Both are tested as refusals rather than as formatting.

### The string-folding defect, a seventh time — now a test

It landed in the new `tests:` line and was caught by reading terminal output,
which is how all seven were caught. Every previous fix added a local
assertion; this one guards the whole report at once: no line may carry a
double space past its label column, checked across probe states, and
mutation-verified by reintroducing a folded string.

## Documentation, and a README that described a different project

v1.0 task item 9. `04-v1.0-harness.md` names two things that must be
unmistakable, and asks for the second "in the docs **and at the approval
prompt**."

**The README was describing a project from several milestones ago.** Its first
paragraph said *"Status: pre-v0.1. The resolver, the agent loop, and model
inference do not [exist]. No recall figure has been measured, and nothing in
this repository reports one."* All four claims are false. That is the same
defect as the stale STATUS entries and the stale `answers.json`, in the one
document a newcomer reads first. Rewritten.

**Coverage is partial, and here is why** now leads the README, before anything
about features: PyCG's ~69.9% recall as the published state of the art, the
trade TRELLIS makes (precision 100% measured, recall 72.0% hand-audited and
explicitly short of the 85% gate), and the fact that it never says "complete."
It shows the actual approval-time line rather than describing it, and repeats
the one warning that matters — `trellis index`'s 43.7% is a lower bound on
recall, not recall.

**`Verified` versus `Reviewed`** gets its own section, and the honest part is
the one that was missing everywhere: **`Verified` is always 0, and that means
"not measured", not "nothing is verified."** Your checks do run and a failing
suite still blocks convergence; what does not exist is the attribution from
tests to call sites. Saying "0 verified" without that label invites exactly
one wrong reading, and it is the reading S3 exists to prevent.

That sentence is now printed **at the approval prompt**, unconditionally —
including when the coverage line is clean, which is precisely when someone is
most likely to read "converged" as "tested".

Held in one `const` that both the printer and its test read. The first version
of that test carried its own copy of the string and would have kept passing
after someone edited the real one — the same failure this sentence exists to
prevent, one level up. Mutation-checked: changing the shipped text fails the
test.

## Packaging, and a proof that only held on one toolchain

v1.0 task item 8.

**The release profile did not exist** — the shipped binary was a cargo
default. `lto = "fat"`, `codegen-units = 1` and `strip = "symbols"` take it
from 6,216,704 to 5,373,440 bytes, 13.6% smaller, for 81 s of build time.

`panic = "abort"` is deliberately not set, and it is the obvious next win.
Unwinding is load-bearing here: `Snapshot::rollback` is proven against an
induced panic with `catch_unwind`, and the TUI restores the terminal from
`Drop`. Neither survives an abort, and a smaller binary is worth less than a
failure path that still works.

**`release.yml` builds three targets**, tests each *on the release profile*
— `lto` and `codegen-units` change what the optimiser may do, and a suite that
only ever sees the dev profile cannot notice — runs `--version` on the result,
and packages it with a SHA-256. Measured at 57 s locally for the release-profile
suite, so the cost is known rather than assumed. `fail-fast: false`, because a
matrix that stops at the first failure hides the other two.

### The compile-fail proof was pinned to a toolchain

Running the suite under WSL to justify the Linux claim found
`tests/forge/struct_literal.rs` failing. The committed `.stderr` expected

```
= note: this error originates in the macro `unimplemented` ...
```

which rustc 1.93 (Windows, here) emits and 1.95 (Linux) does not. So the
compile-fail proof of the `Verdict` seal — a v0.2 gate item, and the thing
that makes the grounding contract structural rather than asserted — **passed
on one machine and failed on another**, and the difference was a note about a
macro that had nothing to do with what was being proven.

Fixed by removing the macro: the private fields are filled with `loop {}`,
which diverges and coerces the same way without contributing a backtrace note.
Verified green on both toolchains. The guarantee under test is field privacy;
the filler should contribute nothing to the message.

### Also found: the manifest claimed a licence with no text

`license = "Apache-2.0"` in `Cargo.toml`, and no `LICENSE` file — so the
release workflow's `cp LICENSE` was silently guarded with `|| true` and would
have shipped binaries claiming a licence whose text was nowhere. Fetched from
apache.org rather than reproduced from memory, with the appendix's copyright
placeholder filled in.

Supported platforms are stated in `dist/QUICKSTART.md` as what was **tested**:
Windows and Linux tested, macOS built in CI only and nothing claimed beyond a
green job. Compiling is not evidence that anything works.

## First run: `trellis init`, and the wall that said no and stopped

v1.0 task item 7.

**`trellis init`** scaffolds `.trellis/config.toml`, `context.md` and
`governance/README.md`, then reports prerequisites with a remedy on each. Two
properties, both tested:

- **Every config value is commented out**, showing the default it would
  override. A scaffold that writes active settings makes the tool's behaviour
  depend on a file the user has not read; this one starts by changing nothing,
  which a test asserts by loading it and comparing against the defaults.
- **The governance example is Markdown, not TOML.** `governance/` loads every
  `*.toml` and a malformed one is a hard error, so an example rule shipped as
  TOML would either take effect or break the repository on first run.

It also needed a change in `main`: root discovery refuses when there is no git
repository, which is right for every other command — `trellis index` in a home
folder once walked 25,147 files — and wrong for the one command whose purpose
is a directory that is not set up yet. `init` falls back to the working
directory and then refuses with the missing step rather than the missing root:
*"this is not a git repository, and TRELLIS rolls back with git. `git init`…"*

**The clean-worktree wall.** It used to state the rule and stop, which leaves
someone holding uncommitted work and a tool that will not start — and the
obvious reading of "requires a clean worktree" is that TRELLIS wants to throw
their changes away. It now says what the requirement buys (rollback is
`reset --hard` to a recorded commit, which cannot tell this run's edits from
work already there, so *your* changes are never at risk from a run) and gives
three ways through: commit, stash, or discard — plus the session's `/diff` when
the changes are a previous run's.

Written as a raw string rather than a continuation-escaped literal, because
the string-folding defect has shipped eight times and a message about a wall
someone has just hit is a poor place for the ninth.

## Degradation UX: the approval moment had nothing

v1.0 task item 4, S6. `trellis radius` already had ADR 0014's two-block
layout — that is the detail view and it was fine. **`/accept` committed with no
coverage information at all**, and a converged one-shot run ended with "changes
are left in the working tree for you to review" and no account of what the
graph could not see in them. The one moment a human decides whether to trust
the work was the one moment TRELLIS said nothing about its own limits.

**Severity, as the doc defines it**: doubt on a symbol nobody is changing is
noise. Repository-wide it is 11,233 sites on poetry — a number whose only
effect is to teach a reader to skip the line. `doubt_in_files` scopes it to the
files being accepted, which on the wedge is *22 in the 1 file*, a figure someone
can hold. Same population caveat as `RadiusCoverage`, over-approximating in the
safe direction, and never divided by anything.

Three paths, and the prompt never blocks: **`a`** accepts and writes the
coverage line into the commit message — which is what "gaps recorded" has to
mean for it to survive, since `git log` is where that question gets asked later;
**`i`** lists the sites with line numbers and enclosing function; **`t`** ranks
those functions by unresolved calls, because "add a test" is not advice and a
named function is. A file with no doubt costs no keystroke, `i` and `t` return
to the prompt rather than deciding, and EOF is not approval.

Two walls found while building it, both the same shape as ones fixed before:

- **`.trellis/` counted as an unexamined source file**, firing the prompt on
  runs with nothing to report. Doubt is now accounted over Python sources only.
- **Nothing ever wrote the gitignore.** A-24 records that `index.bin` *is*
  gitignored — an intention with no code making it true. Untracked, the cache
  dirtied the worktree, and a dirty worktree blocks the next run: a tool that
  writes a cache and then refuses to start because of it has built its own wall.
  `cache::save` writes `.trellis/.gitignore` now, and **the file ignores
  itself**, because TRELLIS writes it unasked and it must not be the reason
  `git status` is dirty either. Verified end to end: indexing a clean
  repository leaves it clean.

## Rollback hardening: seven injection points, two that needed code

v1.0 task items 2 and 3. Full write-up in `docs/bench/rollback-20260920.md`.

**There was no signal handling anywhere in the codebase.** An unhandled SIGINT
killed the process with the run's edits still in the tree — the one state
TRELLIS promises never to leave — and because a run refuses to start on a dirty
worktree, Ctrl-C became a wall for the *next* run too. `Step::Done` is now
raced against `tokio::signal::ctrl_c()`, which is safe to cancel precisely
because of the actor design: the orchestrator is a separate task owning the
only write path, so dropping the loop's future cannot interrupt a write
half-done.

**Verified with a real signal, which took three failed attempts to get
honest.** `kill -INT` from Git Bash on Windows raises no console Ctrl-C event
for a native binary, so the run completed every time and the signal never
arrived — a test that would have passed by never testing anything. It runs
under WSL and on Linux in CI. The mutation confirms it: replace the `ctrl_c()`
arm with a future that never completes and the run converges, exits 0, and
leaves the edit in the tree.

**A dead orchestrator could not corrupt the tree and could waste the whole
budget.** It owns the only write path, so an orchestrator that is gone has
written nothing — but the loop read a dead actor as a model going nowhere.
`Handle::is_alive()` and `NotConverged::OrchestratorGone` stop at the first
unserved call, and the message names this process rather than the user's
prompt.

Three smaller findings:

- **A trail committed *before* a rollback is destroyed by it.** `reset --hard`
  returns HEAD to the snapshot, which is behind the trail commit. `cmd_run`
  already did it in the right order with a comment saying why; there is a test
  under that comment now.
- **A failed rollback swallowed the trail.** `cmd_run` used `?`, so the one run
  whose record matters most — the tree is in an unknown state — was the one
  run that never got a trail written. Exit codes now separate the cases: 1 not
  converged and restored, 2 not restored, 130 deliberate abort.
- **The string-folding defect, a sixth time**, in the rollback-failure message
  itself. Caught by my own assertion — *"no folded indentation in text a user
  reads under pressure"* — which is the first time it has been caught by a test
  rather than by reading output.

## The precision audit, and a stale artifact it found

v1.0's first task item, done. `04-v1.0-harness.md` names three mechanical
checks for gate condition 2; the third did not exist, and it is the one that
reaches the case the other two cannot. `check_soundness` says so itself: *"an
edge to the WRONG `foo` in another module satisfies it."*

`examples/check_reachability.rs` now verifies, over every cross-file `Resolved`
edge, that an import path actually connects the two files — **0 disconnected,
0 ambiguous** on the full corpus, with `1717 + 4157 + 34 = 5908` reconciling
exactly against the recorded `Resolved` count. Full write-up in
`docs/bench/precision-20260920.md`.

Three things worth carrying:

- **The result is reported split by callee shape and refuses to print a pooled
  total alone.** For `Name` the check is close to circular — a scope walk that
  produced a cross-file edge did so *through* an import. For `Attribute` it is
  independent, because receiver typing never consults the calling file's
  imports for the method, and that is exactly where ADR 0012's risk lives.
- **34 edges verify through the class hierarchy rather than an import**, all
  `self.option(...)`. The first run called them disconnected. The check now
  walks base classes requiring each link to be import-connected **and no class
  between the two to define the method itself** — rule 1 of the audit protocol,
  without which the check would bless exactly the edge the protocol calls
  wrong. The 34 are sound by that rule, verified rather than assumed:
  `def option` occurs once in the corpus, at `command.py:46`.
- **Every allowance was removed in turn to prove the check can fail**: without
  the inheritance walk, 34 disconnected; without the one re-export hop, 740;
  with the shadowing guard forced to fire, 34. The guard is wired and reports,
  and on a correct resolver it is inert by design — defence against a future
  change, not evidence about this one.

### The artifact was stale, not the resolver

Scoring `docs/bench/poetry-2.5.0-seed20260919` as stored returned **47.8%**,
the pre-ADR-0012 figure, against 67.9% in every document. The committed
`answers.json` held resolver verdicts from before receiver typing landed, so
scoring that directory replayed history rather than measuring the code.

The resolver had not moved: regenerating the verdicts and scoring them against
the same human judgements gives 67.9%, and 72.0% on seed 20260920. Both stored
files are regenerated. *(I spent a detour convinced this was a CRLF regression
in the resolver — it was my own contaminated scratch directory. The resolver
emits byte-identical verdicts on CRLF and LF trees, which is now checked and
worth knowing given the floor runner's CRLF bug.)*

**This is why the new CI job regenerates rather than replays.** Scoring the
stored pair passes forever, which is precisely the failure
`04-v1.0-harness.md` calls the thing most likely to go wrong: a number from a
spike nobody re-ran. `precision-gate` fetches the corpus by its recorded
SHA-256, runs both mechanical checks, then regenerates each seed's verdicts and
re-scores against the committed human judgements with `--min-recall` and
`--min-precision` floors. `audit_score` previously printed a figure and exited
zero, which is a report and not a gate.

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
| A governance rule fails an audit **mechanically** | **Yes**, as of this iteration, and it was not before — see below. `CheckKind::Governance` produces a sealed `Verdict`; a broken non-negotiable fails the run and rolls it back, and `Step::Done` now audits before converging rather than waiting to be asked. Tested through the dispatch, through the loop, and demonstrated on poetry (`docs/bench/governance-gate-20260920.md`) — not only at the audit function. Substring matching only — a governance check that exercised judgment would be Constraint 2 with extra steps |
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
  where every such file was unreadable **does not pass**, because passing
  there reports compliance that was never established, and `passed()` is what
  convergence reads.

  That rule is keyed on `unreadable`, deliberately not on "read nothing". The
  first version keyed it on an empty `read` and would have failed every run
  that edited a file outside every rule's scope — including every run in a
  repository whose rules are all `deny-write`, which is the wedge fixture's
  own shape. It is the same "a rule broken somewhere this run never went"
  failure the scoping exists to prevent, arriving through the other door, and
  it is now two tests.

### And nothing ever asked for the audit

Fixing the dispatch raised the next question: does a run audit governance at
all unless the model requests it? `wire.rs`:

```rust
fn default_kinds() -> Vec<String> { vec!["typecheck".into()] }
```

It does not. A governance audit ran only when a model volunteered for one,
which makes a non-negotiable rule enforceable at the discretion of the thing it
constrains. No measured model ever asked, and the wedge trail records
`Typecheck- Typecheck+` and no governance check at all.

`Step::Done` now audits before it converges, when the repository has rules —
scoped to what the run touched, recorded as a turn so the trail shows the rules
were consulted, and producing `NotConverged::GovernanceViolation`, which is
separate from `StoppedWithoutPassingChecks` because the user's next move
differs: the code is correct and not allowed. A repository with no rules gets
no audit, no extra turn and no extra passing check — an audit of nothing is not
evidence of anything and must not inflate a count the user reads.

Both directions were checked by mutation rather than by the tests going green:
restoring the shadowed `paths` makes the scope test fail, and stubbing the
rules out of the dispatch makes the violation test fail. The scope test asserts
the read count directly (`1 file(s) read of 1 in scope`) rather than inferring
it from pass/fail, because once the blind rule was narrowed correctly the
mutation passed *and* was still wrong.

Then demonstrated on the benchmark repo rather than left at the unit tests
(`docs/bench/governance-gate-20260920.md`): an edit the type checker accepts and
a non-negotiable rule forbids, the audit appearing as a third step against a
three-step script, the run refusing to converge, the tree rolled back — and the
compliant case still converging, because a gate that cannot pass is not a gate
and this one had two ways to fail closed.

This is the third instance this session of a guarantee documented and tested one
layer below where it was needed, after S20 and the always-empty
`factual.checks`. The pattern is stable enough to name: **test the call the
model makes, not the function it eventually reaches.**

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
