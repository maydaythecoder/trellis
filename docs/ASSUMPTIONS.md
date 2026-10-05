# ASSUMPTIONS

Every default chosen where the brief was silent. Each is a decision that could
reasonably have gone another way, recorded so it can be challenged rather than
inherited.

| # | Assumption | Why | Reversibility |
|---|---|---|---|
| A-1 | Rust edition 2024, `rust-version = 1.93` | Toolchain present on the build machine; edition 2024 gives `let`-chains, used in the parser | Trivial |
| A-2 | Workspace license Apache-2.0 | Brief silent. Permissive, patent grant. Compatible with every dependency (all MIT or MIT/Apache-2.0) | Easy before first release |
| A-3 | `unsafe_code = "forbid"` workspace-wide | No component needs it. tree-sitter's unsafe is behind its own safe API | Easy |
| A-4 | Hand-rolled interner instead of `lasso` | `lasso` upstream stale since 2024-08-19 (verified). Forty lines of surface | Easy — Q-3 |
| A-5 | `postcard` for the index, `bincode` dropped | `bincode` 3.0.0 is a protest release that deliberately fails to compile; 2.x works but the 3.0 squat makes version hygiene a hazard. postcard is MIT/Apache-2.0 and maintained | Easy |
| A-6 | `trellis-vcs` shells out to `git` rather than using `git2` | Brief blesses it. Exact semantics for the path that must never lose work. `git2` is not used; worktrees also shell out through `trellis_vcs` (6b5db2a) | Easy — ADR 0009 |
| A-7 | Budget defaults: 40 iterations, 20 min wall-clock, 2M tokens | **Measured on 8 runs, 2026-10-02, and unchanged**: none of the 8 runs in `docs/bench/cost-by-radius.md` converged (all stopped on tool-call format), so the calibration rule in ADR 0008 (p90 of converged runs + 50%) had nothing to apply to. Still placeholders, not measurements; marked as such in source | Must change — S8, when a measured set contains converged runs |
| A-8 | Default check commands `mypy` and `pytest -q` | Most common in the Python ecosystem. Configured, never detected | Per-repo config |
| A-9 | `trellis doctor` reports tier `ValidateAndRetry` and batching `Unknown` until probing exists | The pessimistic default. Reporting an optimistic guess is exactly the failure S17 and S18 describe | v0.2 |
| A-10 | Recall thresholds 85% gate / 60% pivot | Reasoned from the cost of the unresolved list to the user. See `BENCHMARK_AND_EVAL.md` §3 | Must not change after measuring — that is the point |
| A-11 | Recall sample size 400 call sites | **The stated ±3.5pp was wrong and is corrected.** It assumed the recall denominator equals the sample. It does not: recall is measured over sites whose *true callee is first-party*, which the 2026-09-19 audit found to be roughly 38% of all call sites. 400 draws therefore yield ~150 in the denominator, not 400, and the interval is roughly ±8pp rather than ±3.5pp. **A future audit must set its sample size on the expected first-party count** — about 1,000 draws to put 400 in the denominator | Corrected 2026-09-19 |
| A-12 | `Candidates` counts as **not** resolved for recall | Conservative. The claim is that we name the call sites; naming four possibilities is not naming one | Would inflate the figure — do not |
| A-13 | Recall denominator excludes call sites with a genuinely third-party callee | Pass 2 discards third-party names by design; counting them would understate recall by the third-party share | Recorded per sampled site, so auditable |
| A-14 | Primary benchmark poetry, secondary flask, scale case scrapy | Measured selection, `BENCHMARK_AND_EVAL.md` §1 | Easy before v0.1 |
| A-15 | ADR numbering 0001–0010, with BYOK as 0010 | Brief lists nine ADR topics; BYOK (S12) needed its own and was appended | n/a |
| A-16 | Risk register preserves the brief's numbering, including the absent S13 | Keeps cross-references to the brief valid | n/a |
| A-17 | `Verdict` derives `Serialize` but not `Deserialize` | A derived `Deserialize` is a public constructor from arbitrary JSON and would defeat the seal. Not stated in the brief; it is the brief's intent | Must not change — ADR 0004 |
| A-18 | Trails read back as a distinct `UntrustedTrail` type | S10 says treat trail contents as untrusted on read-back; a separate type makes that visible at every use site rather than remembered | Must not change |
| A-19 | `Coverage::resolved_fraction()` returns `Option`, `None` on an empty sample | "We looked at nothing" and "we resolved everything" must not render identically | Must not change |
| A-20 | Wall-clock is shared, not divided, when budgets split across partitions | Partitions run concurrently, so elapsed time is bounded by the whole ceiling | Easy |
| A-21 | Test fixtures pin `core.autocrlf=false` | Windows git rewrites line endings on checkout, making byte-exact assertions platform-dependent | n/a |
| A-22 | Ground-truth sample seed is recorded in the run artefact | A reproducible sample is auditable; an unrecorded one is not | n/a |
| A-23 | v1.0 supported repository size 100k–300k lines | S9. scrapy at 93.7k sits just under; nothing is claimed above | Widens with measurement |
| A-24 | `.trellis/index.bin` is **gitignored**, not committed | **Deviation from the brief.** Brief 4.4 says the index is "stored in the repo for other agents to read, never merged", which implies committing it. Committing a binary blob that is regenerated on every file change produces a diff on nearly every commit and a merge conflict on nearly every merge, for a file whose stated merge policy is "discard both sides and regenerate". The reader it serves is a process that can rebuild it in ~120 ms on poetry 2.5.0's 81.7k lines (full build, release, 01 §4). Ignoring it costs that rebuild; committing it costs every user every merge | Easy — flip one `.gitignore` line, and update ADR 0007 |
| A-25 | `trellis check` with no paths defaults to `.` | The documented default was "the whole repository"; an empty argv would instead invoke the checker with no arguments, which most tools treat as an error | Trivial |
| A-26 | `src/` is the only synthetic package root stripped when computing a module path | The src-layout convention, used by the primary benchmark. A repository using a different root resolves fewer imports *visibly*, in the coverage report, rather than silently mis-resolving them | Easy — add roots from `pyproject.toml` |
| A-27 | Decorators are **opaque by default**, with an allowlist of transparent ones | **Brief silent, and the choice moves the headline number.** `@foo` may return anything, so a call to a decorated definition cannot be bound without assuming the decorator's semantics. The allowlist holds only decorators whose call-through behaviour is published (`staticmethod`, `lru_cache`, `contextmanager`, …). The opposite default — resolve through every decorator — would raise recall and put unsound edges in the graph, failing v1.0 gate item 2 | Easy to widen; each addition is a soundness claim |
| A-28 | `property` and `cached_property` are **excluded** from that allowlist | They turn attribute access into a call, so `self.x()` invokes the property's *result*, not its body. Resolving it would name the wrong symbol | Must not change |
| A-29 | Re-export chains are followed to depth 4 | Each hop is a real import statement, so following is sound; the limit is about cycles and cost. Deeper chains report `ReexportUnfollowed` rather than guessing | Easy |
| A-30 | **The repository-wide coverage figure is a lower bound on recall, not recall** | An `obj.method()` call whose receiver type is unknown is counted as a miss, but its true callee is frequently third-party (`logger.info`, `session.query`). A-13 excludes those from the denominator — but only an auditor can tell which they are. The repo-wide figure therefore counts calls as misses that the recall definition excludes, and **must not be compared to the A-10 thresholds**. Only the hand-audited sample produces a comparable number | Structural — this is why the audit exists |
| A-31 | Unknown-receiver attribute calls are counted in the denominator as `MethodDispatchUntyped` | The conservative direction: it understates recall rather than inflating it. The alternative — guessing that an unknown receiver is third-party and excluding it — would let the resolver shrink its own denominator to flatter itself | Must not change |
| A-32 | The index skips `.venv`, `venv`, `node_modules`, `build`, `dist`, `site-packages`, cache and `.trellis/work` directories | Vendored and generated trees are not the repository's own names, and `.trellis/work` holds per-partition worktrees that would double every count | Easy |
| A-33 | The audit sample uses a hand-rolled SplitMix64 seeded from the command line | Reproducible from the recorded seed with no dependency whose version could silently change the draw | n/a |
| A-34 | **The v0.1 audit is model-read, not human-read** | The brief and `BENCHMARK_AND_EVAL.md` §2 both say "a human reads call sites". This build's audit was performed by a model reading the blind sample. It is the same protocol and the same blindness, but it is *not* the same evidence: a model auditing a call graph shares failure modes with a static analyser in a way an independent human does not, which is the precise hazard §2 raises about tool-derived ground truth. Labelled as such everywhere the figure appears, and flagged in `OPEN_QUESTIONS.md` | **Must be re-run by a human before the v1.0 gate is claimed** |

## Where this build stopped, and why

This section is the 2026-09-18 state (639ca90), kept as written;
`docs/STATUS.md` holds the current one.

The brief specifies a product through v1.0 and beyond. What exists is the
foundation: the workspace, the structural contracts, Pass 1 of the index,
partitioning rules, rollback, the trail format, and all Part 6 documents.

What does not exist, and is not pretended to: the resolver's Pass 2 and bind
step, the agent loop, model probing, in-process inference, and every empirical
number the brief asks for. Those are v0.1 and v0.2 work, and several of them
(recall, the model floor, the decomposition ratio) are measurements that cannot
be produced by writing code alone.

No fabricated measurement appears anywhere in these documents. Where a number
was needed and could be obtained, it was obtained and labelled with its method;
where it could not, the document says "pending" and names the milestone.
