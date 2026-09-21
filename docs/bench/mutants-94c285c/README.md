# The stopped mutation run — commit `94c285c`

Raw output from the partial Tier 6 run, kept because the triage in
`CORRECTNESS_LEDGER.md` cites it and a triage nobody can check against the
list it came from is an assertion rather than a record.

```
cargo mutants --workspace --no-shuffle -j 4      # rustc 1.93.0, cargo-mutants 27.1.0
tested 842 of ~1253   caught 421 · missed 320 · unviable 93 · timeout 3
```

Stopped at 67%: Claude Code killed the background job because the system was
low on memory while the session was idle. Not a failure of the run.

**Do not read `missed.txt` as a workspace result.** `--no-shuffle` walks
packages in a fixed order, so the 411 mutants never tested are four whole
crates — `trellis-loop`, `trellis-model`, `trellis-trail`, `trellis-vcs` — and
those four carry the highest line coverage in the repository. Every figure
derived from this is biased, and biased low.

It is also **pre-C-001**: `crates/trellis-cli` had no `tests/` directory at
this commit, and 108 of the 320 misses are in `main.rs` alone.

| file | what it holds |
|---|---|
| `missed.txt` | mutants that survived the suite |
| `unviable.txt` | mutants that did not compile — not a test result |
| `timeout.txt` | three, unconfirmed; see the ledger |

The superseding full run is against a later commit and lives in the ledger,
not here. This directory is the record of where the campaign started.
