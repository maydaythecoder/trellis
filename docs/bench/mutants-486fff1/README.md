# The second stopped mutation run — commit `486fff1`

```
cargo mutants --workspace --no-shuffle -j 2      # rustc 1.93.0, cargo-mutants 27.1.0
tested 295 of ~1253   caught 132 · missed 140 · unviable 20 · timeout 0
```

Stopped for system memory pressure, like the first.

## `-j 2` did not help, and that was the point of it

The first run used `-j 4` and reached **842**. This one used `-j 2`
specifically to reduce the memory footprint that stopped it, and reached
**295** — a third as far.

So the stated reason for halving concurrency is disproven. Whatever is
driving the kill is not this job's concurrency, and the change cost roughly
twice the wall time per mutant for no benefit. Recorded because the next
person to reach for `-j 2` should know it was already tried.

## What it establishes: one file, cleanly

Only `trellis-checks` and part of `trellis-cli` were reached. Six crates were
never touched: `trellis-core`, `trellis-index`, `trellis-loop`,
`trellis-model`, `trellis-trail`, `trellis-vcs`.

**There is still no workspace mutation score.**

| file | run 1 (`94c285c`) | run 2 (`486fff1`) | complete in both? |
|---|---|---|---|
| `config.rs` | 3 survivors | 3 survivors | yes |
| `main.rs` | **108 survivors** | **88 survivors** | yes |
| `session.rs` | 54 survivors | 47 so far | **no** — run 2 died inside it |
| `tui/*` | 41 survivors | never reached | no |

`session.rs` is not comparable: run 2's last log is `session.rs_line_727` and
no `tui/` mutant was ever tested, so the file was still in progress.

The percentages are also not comparable, because run 1's `caught.txt` was not
preserved per-file — only its survivors were. Survivor counts are what both
runs support, and they are stated as counts here for that reason.

## `main.rs`: 108 survivors to 88, and which twenty died

The two runs cover this file completely, so the difference is real. C-001 —
`crates/trellis-cli` having no `tests/` directory — is what changed between
them, together with the `needs_repo` fix that suite found.

**Twenty mutants stopped surviving. None started.**

```
replace cmd_check      -> Result<()> with Ok(())
replace cmd_doctor     -> Result<()> with Ok(())
replace cmd_index      -> Result<()> with Ok(())
replace cmd_partitions -> Result<()> with Ok(())
replace cmd_radius     -> Result<()> with Ok(())
replace cmd_resume     -> Result<()> with Ok(())
replace cmd_why        -> Result<()> with Ok(())
replace main           -> Result<()> with Ok(())
replace match guard !needs_repo with true  in main
replace match guard !needs_repo with false in main
replace discover_root  -> Result<PathBuf> with Ok(Default::default())
replace qualified      -> String with "xyzzy".into()
replace qualified      -> String with String::new()
replace resolve_symbol: replace || with &&
replace cmd_run: replace != with ==
replace radius_of: replace > with <
delete ! in cmd_check   (×2)
delete ! in main        (×2)
```

Seven command entry points could have had their entire bodies replaced with
`Ok(())` and the suite would have stayed green. So could `main`. The two
`!needs_repo` guards are the gate that let `trellis model` refuse to run
outside a repository against its own comment — found by the same suite, by
reading rather than by mutation.

**88 still survive**, so this is a first cut and not a solved file. The
remainder is mostly in the parts the integration tests do not reach: the
model-pull path, interrupt handling, and output formatting.
