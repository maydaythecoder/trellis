# ADR 0027 — Drain-run decision log

**Status:** Accepted, appended to during agent runs.
**Date:** 2026-09-28
**Scope:** only decisions an agent may make without the owner
(`docs/BUILD_SYSTEM.md`): reversible by a normal edit, contained to one crate
or module, no change to what a user sees, pays or agrees to, and a defensible
default. Anything else is parked on its issue with a question, not recorded
here.

## Context

The drain run revises the backlog so agents can execute it, then executes it.
Small decisions made along the way would otherwise surface later as surprises.
Each gets one line: what, why, and what would reverse it. Larger ones cite the
issue that holds the full reasoning.

## Decisions

### Revision pass, 2026-09-28

| Issue | Decision | Why | Reverses if |
|---|---|---|---|
| #13 | Remove all 7 added `#[allow]`s; no `#[expect]` in their place | G3 has no allowlist, so a justified suppression still fails the gate | an ADR adds a reviewed allowlist to G3 |
| #14 | Pinning rule: caret requirements, `Cargo.lock` committed as the pin, lock updates in their own commit with deny/audit passing, tree-sitter pair moves together, a new crate needs an ADR | what the repo already does | an ADR adopts exact `=` pins |
| #5 | The dependency re-audit records KEEP/TRIM/SWAP/CUT and applies none; each non-KEEP becomes its own ticket | keeps an s-effort audit from becoming dependency surgery | the owner asks for it applied in one change |
| #26 | Symbol delta in trellis-index (pure), trail matching in `trellis-trail::query`, fixture test in trellis-cli; not wired into prompts | no new crate edge, no dead code; changing what the model sees is ADR 0007's separate step | the owner wants it wired now |
| #106 | Anthropic prompt figure = input + cache_creation + cache_read; llama.cpp uses tokens_evaluated; a partial sum reports unknown | the figure means the same across protocols | #105's answer defines the field differently |
| #145 | "Below the floor" means measured below it; an unmeasured configuration gets no warning | configurations have no order (brief §5) | the owner wants every unmeasured model warned |
| #144 | Floor run model spec `vllm:http://127.0.0.1:11434#qwen3:8b`; report states doctor's tier (ValidateAndRetry) | taken from `spec.rs`'s test and a live doctor run | a constrained-decoding runtime is available |
| #121 #144 #147 | New report files get fixed names; #147 reports prompt size in bytes | so a Verify can name them; trellis does not tokenize | — |
| #132 | `scripts/checks/measured-at.sh`: a document names a commit C with base ≤ C ≤ HEAD plus required text; has `--self-test` | no existing command compares commits | a docs-freshness gate replaces it |
| #32 | Vendor trellis-demo as a git bundle under `qa/fixtures`; CI runs the guards with a plain TCP listener on 11434 | the demo has no remote; `reachable()` is only a TCP connect | trellis-demo gets a remote, or CI gets Ollama |
| #97 | Expose `parse_response` only under `cfg(fuzzing)`, with a workspace `check-cfg` entry | no default-API change; G3 stays clean | a `fuzzing` feature is preferred |
| #149 | Keep `retry_delay`'s signature; a private helper takes the clock; parse IMF-fixdate only | no new dependency | `httpdate` is approved |
| #102–#104 | `insta` from the workspace (1.48.0, already locked), no features | already in `Cargo.lock` | a snapshot needs redactions |
| #35 | `tempfile` as a tui-demo dev-dependency via the workspace | already a workspace dependency | it goes unused |
| #128 #129 | `callers_of`, `callees_of`, `coverage_report` follow `read_note` in BUILTIN_TOOLS, opt-in, not default-on; each ships its doubt (RadiusCoverage counts / enclosed DoubtSites); no `%` | prompt stays byte-identical for repos with no `[tools]`; CLAUDE.md rule 3 | the owner wants them on by default and remeasures |
| #133 | The 500-line SKILL.md cap is a CI check on committed files, not a run-time refusal | no user-visible change; ADR 0019 already bounds run-time cost | the owner wants run-time enforcement |
| #52 | `@` completion: case-insensitive substring, top 10 shortest first, Tab accepts, candidates from `git ls-files` through `trellis_vcs::command()`; `trellis session` gets none | no new dependency; hygiene.rs holds | fuzzy ranking or a rustyline completer is wanted |
| #109 | `/name` stores an optional `Factual::label` (≤ 80 chars, never in PriorContext); `/copy` copies the last `done` summary through clip/pbcopy/wl-copy/xclip with `secrets::scrub`, else prints it | existing optional-field pattern keeps old trail digests; no crate added | a clipboard crate is approved |
| #113 #115 #116 | Steering channel separate from `Human::Channel`; steers stored outside `turns`; tui queues a mid-run line as a steer; session routes a line to an open question first | a steer can never converge a run (ADR 0004); a line is never both | #117 is declined |
| #37 | Split into `*_cmd.rs` modules in 4 ordered steps; a verbatim move is exempt from the 300-line budget | matches `vault_cmd.rs`/`trust_cmd.rs`; `run_body` alone is ~960 lines | the owner wants moves split by function |
| #11 | Rule conflict = exact case-sensitive `Rule.name` substring in the final summary; logic in trellis-loop, nothing under trellis-checks | "keep enforcement dumb"; stays out of the Verdict seal | a one-module edit |
| #74 | The cosmetic not-built list goes in ADR 0021 as a new section | every disposition lives there | it moves to a linked note |
| #101 | `blocked_by` [98, 99, 100] → [98, 99, 28]; `scripts/checks/gate-checked.sh` specified in its Done when | #100 was already done; #28 creates the section #101 checks | — |
| #60 #77–#81 | `blocked_by` corrected to the real dependencies ([98, 21]; [75, 76]) | the old ones named the parent only | — |
| #34 | Verify is `git check-ignore -q chat.log` | exits 0 only when ignored and untracked | a second check is wanted |
| #17 | Relabelled effort:s → effort:m | four per-crate mutation runs | — |
| — | Parked tickets carry `parked` and `triage:needs-human`, and no Done when / Verify | keeps them out of G0 and `next.sh` on two counts | — |
| — | Gates take `--base <ref>`; `verify-tests.sh` fails on zero matched tests; `next.sh` and G0 read eligibility from the issue body, release `triage:blocked` when every `blocked_by` closes, and refuse `parked` | the label-driven queue disagreed with G0; a zero-test filter passes vacuously | — |
| #168 | trellis-index takes serde_json as a normal dependency for the LSP client | already a workspace dependency used at run time by seven crates; no crate added to Cargo.lock | the transport moves to trellis-cli |
