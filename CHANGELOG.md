# Changelog

What each version of TRELLIS holds, newest first. Every entry cites the
ticket that landed it (`#n`, a GitHub issue); the ticket and its commit say
why. The version is `[workspace.package].version` in `Cargo.toml` and
`version` in `desktop/Cargo.toml`, kept equal. How a version is bumped and
released is in `docs/BUILD_SYSTEM.md`, under "Branches".

## [Unreleased]

### Interfaces

- `trellis --web` (ADR 0050 §5, #375): the desktop serves its page and its
  four commands over HTTP on 127.0.0.1 behind a per-launch token, with a
  Host guard against DNS rebinding; `trellis --web` prints and opens the
  URL and stops the server on Ctrl-C, waiting for its rpc child to quit.

### Releases

- Releases are Linux and Windows only, built on the self-hosted runners;
  macOS is dropped until a macOS runner exists (owner decision 2026-10-06,
  ADR 0052). Each release archive carries `trellis-tui`, the Go TUI, beside
  `trellis`, and `publish` refuses unless both report the tag's version
  (#506).

## [0.1.0]

What `main` holds when the owner tags it `v0.1.0`: PR #1 (the v0.1 resolver,
agent loop, rollback, governance, trail and resume, plus the capabilities
inherited from pi, Apex and Paperclip under ADRs 0021-0026), the four
squashed drain runs (PRs #155, #188, #193, #214; tagged `v0.1` at
`747df54`), and every dev-to-main batch through the one that carries #495
and this entry (#386). `v0.1` predates this version scheme: the workspace said
0.0.1 then, and `release.yml`'s `publish` job would refuse it. What is and is
not done, item by item, is in `docs/STATUS.md`.

### Runs and the agent loop

- Multi-agent runs over named targets (ADR 0045, #211): `--agents N|auto` and
  `--target` refused before the snapshot (#465), partitions run together
  (#466, #482), sequential with a reason on a non-batching backend (#483); the
  v1.1 multi-agent exit gate checked off (#467). Staged plans run in
  per-partition worktrees (#98) with an integrated check after they merge
  (#99).
- Rollback re-reads and verifies what the run wrote (ADR 0046, #239).
- Steering and follow-up input between turns (#113), from `trellis tui`
  (#115), `trellis session` (#116) and `trellis rpc` (#114), recorded in
  the trail (#117, #176); `compact` over `trellis rpc` (#110).
- Prompt tokens read from each adapter, recorded in the trail and the budget
  ledger, and printed beside completion tokens (#105-#108, #487).
- Grounded checks bounded by `[checks] timeout_s` and the run budget (#19);
  check output capped at 8 MiB per stream (#171); a check's leftover
  processes killed after it exits (#172).
- A local prompt held to 14,336 bytes (#228); `[model] context_bytes`
  defaults to 400,000 for hosted specs (#152).
- `trellis run --image` on hosted tier 1 (ADR 0041, #53); `trellis resume`
  takes a run id (#25); a run's id is the trail it added (#472, #475).
- The `after_run` hook runs on Ctrl-C and rpc abort (#138); `turn_end` and
  `input` hook events (#137); hooks get a `read_only` fact per built-in tool
  call (#328).

### Models and runtimes

- colibri as a tier-1 runtime over openai-chat (ADR 0047, #251, #252): its
  `/v1/models` and `/health` reader (#261), a tools probe (#262), a request
  profile (#263), queue-wait and model-time fields (#255, #256), and its
  served model, slots and memory tiers in `trellis doctor` (#267). A
  loopback hosted endpoint is identified, and its tools probed if it is
  colibri, before a run's snapshot (#268); the request profile is then on in
  `trellis run` and `trellis ask`, other request bodies unchanged (#270).
- `[model] timeout_s`, read from config and applied to every model request
  (#264-#266).
- Hosted transport: `Retry-After` as an HTTP date (#149), `HTTPS_PROXY` and
  `NO_PROXY` (#150), retry of a connect the OS timed out (#210), chunked
  runtime replies decoded (#229), a reply cut at its length limit named
  (#254), per-model input limits on `[[providers]]` (#162).
- Catalogue models pinned to a commit and SHA-256, verified before use
  (#216).
- Bare `trellis` with no model configured offers locally running models
  (#371) and says one thing everywhere (#376); `trellis work` names a missing
  model the same way (#440); a key refusal names the task whose model chose
  the endpoint (#457).

### Index and resolver

- The Pyright recall backstop behind `[index] backstop` (ADR 0031, #168),
  refusing overridden or abstract callees (#186), with a bounded handshake
  and clearer failures (#187, #445, #448).
- Opt-in `callers_of`, `callees_of` (#128) and `coverage_report` (#129)
  tools, with their doubt attached.
- A repository search never returns a file resolving into `.git` or outside
  the repository (#428); one rule decides whether a model-named path is
  inside it (#441).

### Containment and governance

- `bash` and custom tools (#20), hooks (#174) and process agents (#175) run
  in a throwaway clone and carry their change back; their `.git/` writes stay
  in the clone (#54). Children are spawned into a Windows Job Object (#21).
- Approve mode asks before `write_file` and custom tools, as before `bash`
  (#425).
- Model writes into the configured vault's notes root are refused during
  runs (ADR 0049, #356, #357, #358, #361, #363).
- `trellis install` for skills and prompt templates (#118, #119), over https,
  ssh, git and file only (#179), with skipped paths named (#120) and its tree
  listing bounded (#200); a hook or custom tool that runs a file from the
  packages directory is refused (#201).
- `AI_AGENT=trellis` for bash, custom tools, hooks and process agents
  (#317); `queue note` refused inside a run (#189).
- Labels, paths and note titles escaped before printing (#190, #192, #461).

### Queue, routines and vault

- Queue: wake a profile's task on a person's `@`-mention (#57), opt-in
  release of a dead holder's stale claim (#59) and a block after the third
  release (#352), `[[services]]` beside each task (#60), `queue add --args`
  (#112) and `--model` (#346, #458), `queue cancel --children` (#336),
  `queue list --text` (#338, #453), a task that ends by asking a person
  (#140), a task blocked until a set time (#342), re-runs given a person's
  new notes (#351).
- Routines: IANA zone names in schedules (#47); a scheduled fire gated on
  commits since the last one (#349).
- Vault: Apex's search-result ranking ported (#355, #362); symlinked or
  linked vault files refused (#353, #443); an edit survives a `dates.json`
  failure (#354).

### Interfaces

- How to start (ADR 0050, #369): plain `trellis`, or `--tui`, `--dsk`,
  `--web` (#373); a start screen outside a repository (#372); subcommands
  matched without regard to case (#370).
- `trellis tui`: `@` file search (#52), a budget line with the run's ceiling
  and real elapsed time (#243) and each turn's tokens per second (queue wait
  excluded) and queue wait (#257), a transcript that follows its end (#244).
  The run banner escapes the model spec and other configured text (#492).
  `trellis session`: `/name`, `/copy` (#109) and per-model tokens in
  `/status` (#318).
- `trellis rpc`: read-only `get_codebase`, `get_radius`, `get_queue`,
  `vault_search`, `vault_graph` (#292-#295), `get_session_stats` and
  `get_last_assistant_text` (#319); the fields the desktop reads held by
  tests (#291). A `prompt` sent right after `run_end` is accepted rather
  than refused as a run in progress (#495); #319's stats test no longer
  sends its next prompt before the last run's thread has exited.
- The desktop (ADR 0048, #285): its own workspace, a child module reading
  `get_state` (#287), its static page and icons (#288), and a pure reducer
  from `trellis rpc` frames to its run panes, whose `view` events the core
  prints (#299). Partial: there is no window yet (#289), and the child's
  stderr and delivering `view` events to the page wait for #296.
- `trellis-tickets`: `triage`, `ready`, `new`, `sweep`, `scan` and `index`
  (ADR 0040, #77-#81).

### Measurement and docs

- `run_floor.py` records each turn's model-call time from the run's log
  (#258) and takes `--model-timeout-s` with a harness wait that covers it
  (#273); `bytes_per_token.py --no-runtime-log` reports a run with no
  runtime request log, such as colibri's (#258).
- ADR 0021 gains rows for pi 1.0.2's new capabilities, and row 12.3 says
  doctor's loopback read is on request (#322); stale lines after #441,
  #319, #467, #482, #483, #266 and #267 brought up to date, and
  `gate-checked.sh --self-test` run in the lint gate (#484).

### Build, CI and security

- Branches `main`, `dev` and `audit` (ADR 0051, #378); the pre-push hook
  refuses `main` and `v*` tags (#217, #378, #380, #474).
- Workflows: actions pinned to SHAs, a read-only token, no persisted
  credentials, `--locked` everywhere (#215); the pinned toolchain installed
  (#385); a timeout on every job and one CI run per commit (#381, #477); every
  promotion toward `main` checked (#426); the desktop built, linted, tested
  and audited on Windows and Linux (#297, #384, #290), without Tauri's
  Linux prerequisites, which arrive with Tauri itself (#289).
- Supply chain: cargo-deny and cargo-audit in CI (#14); gitleaks over each
  change's commits (#236); fuzz targets (#95, #97) with a current
  `fuzz/Cargo.lock` (#197); the unused `git2` dependency removed (#424).
- A pre-commit security hook in every worktree and a security-auditor agent
  (#206, #208, #423); drain audited before each merge (#213, #233); scripts
  that move the CI runners to low-privilege accounts (#219).
- Line-by-line review of the whole repository (#391-#422, #429).
- Release: version 0.1.0 for every crate and the desktop, this changelog,
  and a `publish` job in `release.yml` that creates a GitHub release from a
  `v*` tag only when the tagged commit is on `main` and the binary reports
  the tag's version (#386).
