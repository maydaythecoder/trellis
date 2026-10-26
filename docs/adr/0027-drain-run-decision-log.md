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

### Queue run, 2026-10-02 to 2026-10-04

| Issue | Decision | Why | Reverses if |
|---|---|---|---|
| #6 | Regenerated the classify fixture from `ask.rs`'s test at C before running; the classification rows are n=3 (runner default), previous section n=2 | the fixture must be the prompt that ships; the ticket says runner defaults | the owner wants n matched to the previous section |
| #147 | Poetry runs in a fresh git-initialised copy (`.bench/cost`, `core.autocrlf=false`) with `.bench/floor`'s `[checks]` and no `[budget]` | the default budget is what the ticket asks for; autocrlf=true would change bytes on rollback | a committed poetry fixture replaces ad-hoc copies |
| #147 | Every task is a docstring edit in the same file, so the radius is the only variable; ran two rounds for n=2 | holds the work constant across radius sizes | the owner wants tasks that scale with the radius (renames) |
| #147 | Scoped check timed on the file every task targeted, since no run touched a file | the ticket's "files the run touched" was empty | — |
| #15 | Stopped the Windows Ollama daemon for the pass and ran WSL's; panic induced by a one-line F12 probe in a throwaway clone and separate target dir only | WSL cannot reach the Windows daemon in NAT mode, and two daemons contend for the GPU; the binary under test stays at C | — |
| #17 | Rule's "re-run" branch not taken for the four crates over the timeout limit; counts, no score | the timeouts are non-terminating-loop mutants, which a re-run reproduces | a per-test time limit makes them scoreable |
| #17 | One issue per crate (#245-#248) as the disposition for every survivor except one shown unexercisable; none judged equivalent | 847 equivalence claims could not be made carefully in one sitting | — |
| #122 | Widened the hygiene `plain()` guard to scan all of `src/`, in its own commit before the move | the guard read `main.rs` by path and would have stopped seeing `cmd_log`/`cmd_why` | — |
| #124 | Moved the unlisted run-path types and helpers with the listed functions; re-exported names other modules reach as `crate::X` instead of editing their paths | the ticket's intent is "leaving main.rs with argument parsing, dispatch and root discovery"; re-exports keep the move commit to mod/use/visibility lines | the owner wants callers updated by path |
| #126 | Compared mutant by mutant (function + mutation, `file:line:col` ignored) instead of by caught fraction | both runs exceed the timeout limit, so neither has a fraction; the split moved files | — |
| #194/#195/#196/#201/#211/#221/#239 | Recorded each owner answer as a `## Decision` section and moved the issue from parked/needs-human to triage:auto (unparker procedure); #219 relabelled owner-task with steps, #220 blocked on #219 | the answers came from the owner in the session; the issue body is where G0 reads them | — |
| #239 | Not merged by the session: risk:high under #196's new path table, so it waits for the owner's approval even though its labels did not trigger G7 | the label predates the table; the table is the rule | the owner says merge |
| #242 | Of three example wordings scoring 15/15, kept the one that still says what a change does | the point of the change was the way forward, not brevity | a later measurement shows it costs empty replies |
| #243 | The loop emits one `run budget` event per run; the TUI reads the ceiling from it rather than from config | the hosted path can differ from the config, and the loop's figure is the one enforced | — |
| #244 | Scroll state counts lines back from the end, with the outcome on the transcript's non-scrolling border | following is the default and needs no layout in the state | — |
| #245 | A nextest `mutants` profile (60 s x 3, four threads) used only when NEXTEST_PROFILE=mutants; a starved run (over 40% unviable) is not recorded, and two starved results were discarded | turns hung mutants into failures without touching cargo test or CI | the owner prefers a different tool |
| #249 | `desktop/**` maps to area:tui | the desktop is an interface; #194 keeps the area set fixed | the owner adds a desktop area |
| plan | 49 owner questions in the 2026-10-04 upstream plan answered with each ticket's recommended option, recorded in the ticket's `## Decision`; 32 tickets routed from needs-human to triage:auto where a rule abstained on an input an issue does not carry | the owner delegated every open decision ("ready for completion without decisions") | the owner edits any one ticket's Decision |
| #638 | A desktop page that loaded after the core emitted `no-child` or `rpc-exited` takes `rpc_send`'s no-child refusal of its `get_state` as the not-running state, matched on the refusal's prefix (a test pins it to `commands.rs`) and dropped when a `child-started` arrived after the line was sent | the core emits those events once, before a late page listens, and no new command, event or capability is needed | the core resends the child's state to a page that asks |
| #656 | The desktop overview draws a queue task's `status`, the board's word, and `board.tasks[].status` joins the overview's `always` list in `client_contract.json` beside `state` and `task` | ADR 0048 §6 lets a client rely only on listed fields; listed, a rename fails trellis's own contract test instead of blanking the word | the owner keeps the overview's contract to what #558 listed |
| #799 | The wall-clock ceiling is `max_wall_clock_ms`, in the unit of `elapsed_ms`, not `[budget] max_wall_clock_s`'s seconds; the token counter is `completion_tokens_used`; `run_end`'s `elapsed_ms` is written by the loop only when it returns (a `Seed::spent` slot), so an interrupted, aborted or partitioned run's `run_end` has none | a client compares elapsed to the ceiling in one unit; the budget counts completion tokens only (UI rule 5); a figure taken at the last event before an interrupt would be part of the run sent as all of it | ADR 0058's `set_budget` (#807) is built to take seconds, or the owner wants the last-measured figure on an interrupted `run_end` |
