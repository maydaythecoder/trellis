# Shard 7/10 of `trellis-cli` at 6b804cb, from the capped re-run at d741a5e

The four outcome files of `mutants.yml` run 41462469276
(`crate=trellis-cli os=linux shard=7/10`, dispatched 2026-11-02 at the
branch `t248-mutants-6b804cb-cap`, commit `d741a5e`), unchanged from the
artifact `mutants-trellis-cli-linux-d741a5e-shard-7-of-10` (id 13173516257):
`252 mutants tested in 5h: 18 missed, 227 caught, 6 unviable, 1 timeouts`,
which the four files' line counts match (the step ran 5h09m).

Why `COMMIT` reads `6b804cb` when the artifact names `d741a5e`: the owner's
decision of 2026-11-03 on #995. `d741a5e` is `6b804cb` plus one commit that
touches only `.github/workflows/mutants.yml` (`git diff --stat 6b804cb
d741a5e`: 1 file), the memory cap of #1026; `cargo mutants --list -p
trellis-cli --no-shuffle --shard 7/10` is the same at both commits (#995,
comment of 2026-11-02). The three uncapped dispatches of this shard
(41263109773, 41290109608, 41307704470) lost the runner and uploaded
nothing.

Of `start_screen.rs:261:5`'s replacements of `answer`, the one whose test
allocates without bound in the uncapped runs, `Ok(Some(String::new()))`, is
in `caught.txt` here, as is `Ok(None)`; `Ok(Some("xyzzy".into()))` is in
`timeout.txt`.
