# Shard 2/10 of `trellis-cli` at 6b804cb, from the capped re-run at d741a5e

The four outcome files of `mutants.yml` run 41490519271
(`crate=trellis-cli os=linux shard=2/10`, dispatched 2026-11-02 at the
branch `t248-mutants-6b804cb-cap`, commit `d741a5e`), unchanged from the
artifact `mutants-trellis-cli-linux-d741a5e-shard-2-of-10` (id 13179117888):
`252 mutants tested in 4h: 23 missed, 196 caught, 26 unviable, 7 timeouts`,
which the four files' line counts match (the step ran 3h46m).

Why `COMMIT` reads `6b804cb` when the artifact names `d741a5e`: the owner's
decision of 2026-11-03 on #995. `d741a5e` is `6b804cb` plus one commit that
touches only `.github/workflows/mutants.yml` (`git diff --stat 6b804cb
d741a5e`: 1 file), the memory cap of #1026; `cargo mutants --list -p
trellis-cli --no-shuffle --shard 2/10` is the same at both commits (#995,
comment of 2026-11-02). The three uncapped dispatches of this shard
(41263101036, 41290108034, 41307702937) lost the runner and uploaded
nothing; a first capped run (41462454920) was cancelled at 125 of 252.

`first_run.rs:316:40: replace == with != in pick`, the mutant whose test
allocates without bound, is in `timeout.txt` here: cargo-mutants' 300 s
limit ended it in this run. In the cancelled run 41462454920 the cap's
SIGKILL ended it first and it was recorded as caught; these files are
41490519271's, as they were uploaded.
