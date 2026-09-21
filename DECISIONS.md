# DECISIONS

Non-obvious choices, one line each: date, decision, why, what would reverse
it. Lighter than an ADR — an ADR is for a decision that is expensive to undo,
and the ten in `docs/adr/` are those. This file is for the rest.

**This file was created late.** The session prompt's §5 has required it
throughout, and the entries below the line marked *backfilled* were
reconstructed from commit messages and ledgers rather than written when the
choice was made. Reconstruction is weaker than a record: it captures what was
decided and why, and it cannot capture what was considered and dropped. Every
entry from 2026-09-21 onward is written at the time.

| Date | Decision | Why | What would reverse it |
|---|---|---|---|

## Backfilled — reconstructed 2026-09-21

| Date | Decision | Why | What would reverse it |
|---|---|---|---|
| 2026-09-21 | Toolchain pinned to rustc 1.93.0 (`rust-toolchain.toml`) | Mutation scores, coverage percentages and `trybuild` stderr are all toolchain-sensitive; the `Verdict` compile-fail proof had already broken once because 1.93 emits a macro backtrace note 1.95 does not | A CI matrix that tests several toolchains, which would need the compile-fail proof made version-agnostic first |
| 2026-09-21 | `deny.toml` licence allow-list read from the lockfile, not copied from a template | Every expression in it is one a crate in this tree actually carries, so the list says something true about this project rather than about a generic one | A dependency arriving with a licence not in the list — which is the point: it fails, and someone decides |
| 2026-09-21 | `RUSTSEC-2023-0089` ignored with a stated expiry condition | `atomic-polyfill` is unmaintained, not vulnerable, and `cargo tree -i` shows it compiled for neither supported target | postcard taking a heapless 0.8 dependency, or TRELLIS supporting a target without native atomics |
| 2026-09-21 | The eight crates marked `publish = false` | The deliverable is one binary, not eight libraries. Also stops `cargo deny` reading intra-workspace path dependencies as registry wildcards | Deciding to publish `trellis-index` as a standalone Python call-graph library, which nothing currently proposes |
| 2026-09-21 | Narrowing the touched-file partitionability test (ADR 0016 has the full record) | Unit partitionability was 2.9% and the decomposition ratio 4.0%, which made v1.1 worth almost nothing | It is an ADR because it is expensive to undo — see there |
| 2026-09-21 | `Interner` serialises its store only, rebuilding `lookup` on load | The map is a second copy of every key, and `HashMap` iteration order differs between instances, so `.trellis/index.bin` differed on every rebuild of unchanged sources. 16% smaller and byte-reproducible | An interner large enough that rebuilding the map on load is measurably slow, which at 3,735 symbols it is not |
| 2026-09-21 | `unreachable_pub` enabled workspace-wide, and nineteen `pub mod` narrowed to `mod` | `pub` on an item nothing outside the crate can reach is a claim about API that is not true. Narrowing also made the compiler find dead code the earlier read-through had walked past | A crate here becoming a published library, at which point its module paths are the API |
| 2026-09-21 | `trellis_loop::tools::args` deleted rather than wired up | Five argument structs used by nothing — an unwired design that `ToolCallWire` superseded before anything called them | A per-tool grammar becoming necessary, which tier 3 would have needed and ADR 0013 descoped |

## Current

| Date | Decision | Why | What would reverse it |
|---|---|---|---|
| 2026-09-21 | Mutation testing run **per crate** rather than across the workspace | Two whole-workspace runs were killed by the system for memory at 842 and 295 of ~1253 mutants. `trellis-loop` alone finishes in 12 minutes, and saving after each crate means a kill costs one crate rather than everything | A machine with headroom for the whole workspace, or cargo-mutants gaining a resume flag. The per-crate results are equivalent, so this is a scheduling choice and not a measurement one |
| 2026-09-21 | `-j 2` tried to reduce memory pressure, and **abandoned** | It reached a third as far as `-j 4` at roughly twice the wall time per mutant. Whatever drives the kill is not this job's concurrency | Nothing — this is a negative result, recorded so it is not tried again. `-j 1` is not the next thing to try |
| 2026-09-21 | Quality audit Passes 3, 4, 5 and 7 done without the mutation baseline; Pass 6 and the `main.rs` split held for it | The audit's §9 rule exists to catch a refactor that makes code *harder to test*. Named constructors, visibility narrowing and a lint census cannot do that — narrowing made the compiler find dead code, which is the opposite. Pass 6's extraction can, which is what the rule is for | The baseline landing, which unblocks the two held items rather than revisiting the four done ones |
| 2026-09-21 | Pass 6's git-fixture **drift fixed**, its **extraction held** | Two of seven fixtures had lost `core.autocrlf false`, which is a defect and was fixed. The extraction would unify five of seven — `main.rs` and `session.rs` share a crate; `tests/cli.rs` and `trellis-vcs` do not — and a change reaching five sevenths of a class is worth making with a score in hand | The mutation baseline |
| 2026-09-21 | splitmix64 left duplicated across the three measurement examples | Six lines of a named standard algorithm in three standalone tools each meant to be read in isolation. The alternative is an RNG in `trellis-core`, whose charter is ids, the interner and errors | A fourth measurement tool, or one of the three drifting — the risk the duplication carries is real and only the cost of fixing it is currently higher |
