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
| A-6 | `trellis-vcs` shells out to `git` rather than using `git2` | Brief blesses it. Exact semantics for the path that must never lose work. `git2` retained for worktrees | Easy — ADR 0009 |
| A-7 | Budget defaults: 40 iterations, 20 min wall-clock, 2M tokens | **Placeholders, not measurements.** Marked as such in source | Must change — S8 |
| A-8 | Default check commands `mypy` and `pytest -q` | Most common in the Python ecosystem. Configured, never detected | Per-repo config |
| A-9 | `trellis doctor` reports tier `ValidateAndRetry` and batching `Unknown` until probing exists | The pessimistic default. Reporting an optimistic guess is exactly the failure S17 and S18 describe | v0.2 |
| A-10 | Recall thresholds 85% gate / 60% pivot | Reasoned from the cost of the unresolved list to the user. See `BENCHMARK_AND_EVAL.md` §3 | Must not change after measuring — that is the point |
| A-11 | Recall sample size 400 call sites | ±3.5pp at 95% near a true recall of 0.85; thresholds are 25pp apart | Easy |
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

## Where this build stopped, and why

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
