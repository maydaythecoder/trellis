# Build system: orchestrator, agents, gates

How trellis is built by agents with a person in the loop only where a decision
is theirs. It is trellis's own thesis applied to building trellis: agents
parallelize work, never judgment (ADR 0005), and a pass comes only from a
command that ran and exited 0 (ADR 0004). No agent's claim that something works
counts for anything; the gates do.

## Starting it

From the repository root:

```
claude --agent orchestrator
```

That session *is* the orchestrator. Tell it what to do ("work the queue",
"do #14", "why is #2 not ready"). It stops on its own after 5 tickets, three
failures on one gate, or a ticket that needs a person (G7), and reports.

## Parts

| Part | Where | Does | Cannot |
|---|---|---|---|
| orchestrator | `.claude/agents/orchestrator.md` | picks tickets, dispatches, runs gates, merges into `drain` | write or edit files; push to `v0.1-resolver` |
| architect | `.claude/agents/architect.md` | ADRs, `## Done when` / `## Verify` on tickets | write application code |
| implementer | `.claude/agents/implementer.md` | one ticket, on its branch, with tests | decide architecture; add deps, `#[allow]`, `unsafe` without an ADR |
| verifier | `.claude/agents/verifier.md` | runs gates, reports output verbatim | edit anything; give an opinion |
| security | `.claude/agents/security.md` | read-only review of trust-boundary changes | edit anything |
| ops | `.claude/agents/ops.md` | CI, `deny.toml`, `scripts/gates`, `scripts/tickets` | touch crate code |
| unparker | `.claude/agents/unparker.md` | ranks parked / needs-human tickets by what an answer unblocks, drafts one question each for the owner, applies the answers until a ticket needs nobody | decide for the owner; ask directly (the coordinator asks) |
| queue-runner | `.claude/agents/queue-runner.md` | works the agent-executable queue one ticket at a time: implement, gate, merge into `drain`, close | touch parked work; push to `v0.1-resolver` |
| queue | GitHub issues + Trellis roadmap project (#2) | the tickets, labelled per the ticketing standard | |
| gates | `scripts/gates/` | exit 0 or say why not | be skipped |

Subagents cannot spawn subagents, so only the orchestrator dispatches.

## Gates

`scripts/gates/run-all.sh [<issue>]` runs them in order and stops at the first
failure, printing `GATE / COMMAND / EXIT / LOG / OUTPUT`. `--only <id>` runs one.
`OUTPUT` is the last 40 lines; `LOG` is the gate's full output, in
`$GATE_LOG_DIR` (default `${TMPDIR:-/tmp}/trellis-gates/<HEAD sha>/<gate>.log`,
and never inside the repository). A G2 failure also prints every
`---- <test> stdout ----` block and `panicked at` line from the full log (#178).
"This change" means `$GATE_BASE...HEAD`: `origin/drain` on a ticket branch
(`t<n>-*`), `origin/v0.1-resolver` otherwise (`drain`'s own PR diff).

| Gate | Script | Proves |
|---|---|---|
| G0 | `ticket-ready.sh <n>` | the issue is open, has `## Done when` and a fenced `## Verify`, is not blocked, needs-human, or split |
| G1 | `build.sh` | workspace builds, all targets; `trellis-cli --no-default-features` builds |
| G2 | `test.sh` | full workspace suite, and the local-only CLI suite; each `cargo test` fails after `GATE_TEST_TIMEOUT_S` seconds (default 2400) rather than hanging (#173) |
| G2b | `guards.sh` | `qa/run-guards.py` holds every guard against the fresh debug binary, and fails unless `trellis` resolves to it; needs Ollama at 127.0.0.1:11434. Before #158, on Windows, it ran whatever `trellis` was installed instead |
| G3 | `lint.sh` | `cargo fmt --check`, `clippy -D warnings`, and no `#[allow(` added by this change |
| G4 | `no-stubs.sh` | no `TODO`/`FIXME`/`todo!`/`unimplemented!`/mock markers in production lines this change added (test modules, `tests/`, `examples/` excluded) |
| G5 | `security.sh` | `cargo deny check`, `cargo audit`, no key-shaped literal added |
| G6 | `ticket-verify.sh <n>` | the ticket's own Verify command exits 0 |
| G7 | in `run-all.sh` | exit 3 when the ticket is `risk:high` or `surface:public`: a person approves the merge |

Every gate unsets provider keys before it runs anything (ADR 0024 §3).
G6 runs text from an issue body, so each line must start with `cargo`, `git`,
`scripts/`, `py -3 qa/` or `python3 qa/`, and may not contain `..`, `$(`,
backticks, pipes, `;`, `&` or redirection. `python3` and `py -3` both run as
whichever Python the machine has.

A `blocked_by` entry that is not an issue number blocks the ticket (G0 and
`next.sh`): a dependency the scripts cannot look up is not a satisfied one. Anything more belongs in a script under
`scripts/`, where it is reviewed as code.

A gate that cannot run fails. There is no skip variable and no `--force`; a
wrong gate is fixed in its own ticket.

### State at introduction

Run on `inherit` (so against PR #1's whole diff) at the commit that added
this file; the output is in the commit message.

- G1, G2, G2b (11/11 guards), G4, G5 pass.
- G3 fails on the seven `#[allow]` lines PR #1 added (issue #13). The gate's
  first version matched three: its pattern missed a suppression at column 0.
  Ticket branches are measured against `inherit`, so this does not block them.
- G5 first failed on a key-shaped test canary in
  `crates/trellis-cli/tests/hosted_keys.rs`; that commit builds it at run time.
- G0 fails on every existing ticket until the architect adds `## Done when`
  and `## Verify`; that is the orchestrator's first job.

## The loop

1. `scripts/tickets/next.sh` → the next ready ticket (`--why` explains every
   exclusion).
2. G0. Failing → architect makes it workable → G0 again.
3. Branch `t<n>-<slug>` off `drain`; dispatch per the routing table.
4. `run-all.sh <n>`. A failure becomes a fix ticket quoting the output; three
   failures on one gate stops the run.
5. Green (and G7 approved) → `git merge --no-ff` into `drain`, push, close the
   issue with the gate summary.

`v0.1-resolver` moves only when the owner merges a pull request on GitHub.
Agent work collects on the integration branch `drain`, which has an open
draft PR into `v0.1-resolver`. The local pre-push hook refuses every push to
`v0.1-resolver` from this clone (GitHub branch protection needs Pro on a
private repository; issue #22).

## Where a person is needed

Only where the decision is theirs, not where the work is hard:

- tickets labelled `triage:needs-human` (trust-boundary risk: `area:checks`,
  `area:protocol`, `!security`; or an owner decision);
- G7 approval before merge;
- promoting a project item from Draft to Todo;
- anything touching API keys, deleting data, or irreversible;
- resuming after a three-strike stop.

## Adapted from the generic template, and what was dropped

The source template targets a greenfield product. Kept: one orchestrator, the
specialist roster, shell gates, three strikes, human approval on high risk.
Changed or dropped:

- **Queue**: GitHub issues and the existing ticketing standard instead of a
  `tickets/` directory, so the board, triage and the agents read one source.
- **Stack bootstrap** dropped: the stack exists (Rust workspace, CLAUDE.md
  conventions, CI).
- **"Sellable" bar and launch checklist** dropped: trellis's bar is its own
  gates in `BENCHMARK_AND_EVAL.md` and `STATUS.md`, with measured figures or
  "pending"; a second, vaguer bar would compete with them.
- **Web-app items** (e2e browser tests, accessibility, bundle size, uptime
  monitoring) dropped: trellis is a CLI with no server.
- **Coverage threshold** not added as a gate: coverage is measured
  (issue #17) but a percentage gate rewards tests written for the number.
- **Secret scanner**: a key-shape regex over added lines stands in until a
  real scanner is installed; GitGuardian still runs on the PR.

## CI runners

GitHub-hosted jobs stopped starting on 2026-09-28 ("recent account payments
have failed or your spending limit needs to be increased"). CI runs on two
self-hosted runners on the owner's machine instead; a smoke job on one ran
while GitHub-hosted jobs were blocked (run 36507327810).

| Runner | Where | Labels | Started by |
|---|---|---|---|
| `trellis-win` | `C:\actions-runner\win` (Windows host) | self-hosted, Windows, X64 | `start-runner.cmd` there |
| `trellis-wsl` | `~/actions-runner` in WSL2 Ubuntu 24.04 | self-hosted, Linux, X64 | `wsl.exe -d Ubuntu -- ~/actions-runner/start.sh` |

- Each `runs-on` in `ci.yml` reads `vars.LINUX_RUNNER` / `vars.WINDOWS_RUNNER`
  (repository variables holding a JSON label list) and falls back to
  `ubuntu-latest` / `windows-latest` when unset. Back to GitHub-hosted:
  `gh variable delete LINUX_RUNNER` and `gh variable delete WINDOWS_RUNNER`.
- Each runner's `.env` gives jobs their own `CARGO_HOME`, `RUSTUP_HOME` and
  `GIT_CONFIG_GLOBAL` under the runner's directory. Without them a job runs as
  the owner with the owner's homes: on 2026-09-28 `Swatinem/rust-cache`'s
  post-job cleanup of `$CARGO_HOME/bin` deleted rustup and every installed
  cargo tool on both machines, and `ci.yml`'s `git config --global` steps
  would have rewritten the owner's git config (#158). rust-cache now runs on
  GitHub-hosted runners only. Each launcher strips the provider-key variables.
- The runners are processes, not services: they stop on logout or reboot and
  are restarted with the commands above. One job runs per runner at a time.
- WSL runs its own Ollama on 127.0.0.1:11434, so the `guards` job's stand-in
  listener does not bind; the job only needs something listening there.
- Self-hosted runners run whatever a pushed workflow says, as the owner. That
  is acceptable for a private repository with one owner; GitHub advises
  against it for public ones.

## CI without GitHub

`py -3 scripts/ci-local.py [--ref <commit>] [--job <name>]... [--keep-going]`
runs every job in `.github/workflows/ci.yml` on this machine: Linux legs in
WSL, Windows legs natively, one at a time, each in a clean checkout of the
commit under `C:\ci-local` (or `~/ci-local` in WSL). `uses:` steps are
replaced (checkout, toolchain) or skipped (rust-cache); setup-python becomes a
virtualenv; an action it does not know fails the job. Jobs get a throwaway
`GIT_CONFIG_GLOBAL` and `CARGO_INSTALL_ROOT`, no provider keys, and a closed
`OPENAI_BASE_URL`. Exit 0 only when every leg passed. It posts nothing to
GitHub; the self-hosted runners do that.
