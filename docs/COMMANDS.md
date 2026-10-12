# Commands

Every command trellis and its tooling take, each with the ADR that added it.
CLAUDE.md keeps the everyday ones; this is the full list.

```
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets
cargo fmt --all

cargo run -p trellis-cli -- index          # full index: parse, resolve, coverage
cargo run -p trellis-cli -- radius <sym>   # blast radius with confidence breakdown
cargo run -p trellis-cli -- check          # run the grounded checks
cargo run -p trellis-cli -- doctor         # probe tier, batching, tooling

# owner-directed growth beyond brief 4.12, each recorded in its ADR
cargo run -p trellis-cli -- run "<task>" --focus <stem>    # vault note as context (ADR 0017)
cargo run -p trellis-cli -- run "<task>" --approve|--auto  # the control dial (ADR 0018)
cargo run -p trellis-cli -- queue add "<task>" [--focus <stem>]   # (ADR 0020)
cargo run -p trellis-cli -- work [--once | --every <s>] [--max-tasks N]
cargo run -p trellis-cli -- vault check|search|graph|new|reparent|link|retitle|rm|init|dates   # (ADR 0022)
cargo run -p trellis-cli -- trust [--show | --revoke]      # hooks, custom tools, bash (ADR 0023)
cargo run -p trellis-cli -- run "<task>" --model anthropic:https://api.anthropic.com#<model>  # hosted (ADR 0024)
cargo run -p trellis-cli -- run "<task>" --json            # JSONL on stdout (ADR 0025)
cargo run -p trellis-cli -- rpc                            # JSONL commands on stdin (ADR 0025)
cargo run -p trellis-cli -- [--root <repo>] [--json] overview   # recent repos' last outcomes, last run, codebase, board; read-only, never indexes; owner 2026-10-07 (#557); as built 2026-10-07 (#558): needs no repo, gh read-only and bounded to 10 s, exits 0 without it (ADR 0050 §3, ADR 0025 §4)
cargo run -p trellis-cli -- agents list | routines list    # profiles, routines (ADR 0026)
cargo run -p trellis-cli -- resume [<run_id>]              # a named run's trail, as data (ADR 0038)
cargo run -p trellis-cli -- install <source>@<ref> | --list | --remove <name>   # skills and templates only; --list ends each line with `digest ok`, `modified: <paths>` or `not recorded` (ADR 0039 §4, #617)
cargo run -p trellis-cli -- install --upgrade <name>       # re-resolves the recorded ref over the old copy; refused over local edits with the paths named, no --force; same commit is a no-op (ADR 0039 §11, #618)
cargo run -p trellis-cli -- install <source>@<ref> --dry-run   # fetches and lists what would be copied and skipped, then "would install N skill(s), M prompt(s)"; packages/ and packages.toml untouched (ADR 0039 §12, #619)
cargo run -p trellis-cli -- run "<task>" --image <path>    # hosted tier 1 only, repeatable (ADR 0041)
cargo build -p trellis-cli --no-default-features           # local-only binary, no TLS (ADR 0024)
cargo test -p trellis-tickets                              # ticket label rules (docs/TICKETING.md, ADR 0040)
cargo run -p trellis-tickets -- triage <n> [--json]       # one issue's labels, each with its rule; read-only (ADR 0040)
cargo run -p trellis-tickets -- ready <n>                 # G0's checks + 4 named extras; exit 1 lists each failure or abstention; read-only (ADR 0040)
cargo run -p trellis-tickets -- new --title <t> --body-file <p> [--dry-run]   # files one issue with triage's labels; refuses a body G0/G6 would; prints the plan first (ADR 0040)
cargo run -p trellis-tickets -- sweep [--dry-run|--apply]  # label drift on every open issue; writes only with --apply; skips parked/split; never ready, !agent-ok, or a lower risk (ADR 0040)
cargo run -p trellis-tickets -- scan [<path>]              # G4's markers in Rust source as file:line proposals; files nothing (ADR 0040)
cargo run -p trellis-tickets -- index [--out <p>]          # open issues' triage as JSON, triage_version 2, by story then number; same issues, same bytes (docs/TICKETING.md, ADR 0040)

# ADR 0021 is the map: every pi, Apex and Paperclip capability, and whether
# it is built, deferred, or excluded (and by which clause).

# qa/run-guards.py runs whichever `trellis` is on PATH, and dirty_worktree
# needs the demo's runtime up: put target/debug first and start the runtime,
# or the guards test an old binary and one refuses for the wrong reason.

# v0.1 measurement tooling (examples, not CLI surface — the surface is fixed
# by brief 4.12 and does not grow to suit us; growth the owner directs is
# recorded in an ADR, as 0017, 0018, 0020 and 0021-0026 do)
cargo run --release -p trellis-index --example audit_sample -- <root> <n> <seed> <dir>
cargo run --release -p trellis-index --example audit_score  -- <dir>
```
