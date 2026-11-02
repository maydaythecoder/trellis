# `file=crates/trellis-cli/src/run_cmd.rs` at 6b804cb

The four outcome files of `mutants.yml` run 41290111656
(`crate=trellis-cli os=linux file=crates/trellis-cli/src/run_cmd.rs`,
dispatched 2026-11-01 at the pinned branch `t248-mutants-6b804cb`), unchanged
from the artifact `mutants-trellis-cli-linux-6b804cb-file-run_cmd`:
`235 mutants tested in 4h: 26 missed, 168 caught, 40 unviable, 1 timeouts`
(the step itself ran 3h52m; "4h" is cargo-mutants' own rounding).

Why it was run (ADR 0060, "Failure modes"): shard 5/10 of the same run
stopped at the step's 345-minute limit with 207 of 252 mutants tested, and the
33 `run_cmd.rs` mutants in its partition were never reached. Shard 5's own
files stay under `../shard-5-of-10/` as its artifact wrote them; this
directory is what finishes the shard's measurement for that file. The 12
`rpc.rs` mutants of shard 5 that were also unreached have no outcome in any
run; whether to spend ten `file=rpc.rs` shard jobs on them is the owner's
decision (#248, comment of 2026-11-01).

A file dispatch runs every `run_cmd.rs` mutant of the crate, not only shard
5's 33, so these files overlap shard 6. Every `run_cmd.rs` line of
`../shard-6-of-10/`'s four files, 202 of them, appears here with the same
text, and the 33 lines here that shard 6 does not hold are shard 5's unreached
33 (`comm` on the sorted lines; 235 − 202 = 33): 20 in `caught.txt`, 7 in
`missed.txt`, 6 in `unviable.txt`, none in `timeout.txt`. Of the 27 survivors
here (26 missed, 1 timed out), the 20 that are shard 6's are dispositioned
under #1005; the 7 that are shard 5's (`Driver::tokens` ×3,
`Driver::prompt_tokens` ×3, `Human::terminal`) belong to shard 5's batch
ticket.

`scripts/checks/mutants-ledger.sh --all` enumerates only the
`shard-<k>-of-10` directories, so the 7 shard-5 lines here are checked only
by running the script on this directory. Shard 5's batch ticket names it in
its Verify beside `shard-5-of-10`.
