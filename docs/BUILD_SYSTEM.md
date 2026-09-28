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
| orchestrator | `.claude/agents/orchestrator.md` | picks tickets, dispatches, runs gates, merges into `inherit` | write or edit files; merge into `v0.1-resolver` |
| architect | `.claude/agents/architect.md` | ADRs, `## Done when` / `## Verify` on tickets | write application code |
| implementer | `.claude/agents/implementer.md` | one ticket, on its branch, with tests | decide architecture; add deps, `#[allow]`, `unsafe` without an ADR |
| verifier | `.claude/agents/verifier.md` | runs gates, reports output verbatim | edit anything; give an opinion |
| security | `.claude/agents/security.md` | read-only review of trust-boundary changes | edit anything |
| ops | `.claude/agents/ops.md` | CI, `deny.toml`, `scripts/gates`, `scripts/tickets` | touch crate code |
| queue | GitHub issues + Trellis roadmap project (#2) | the tickets, labelled per the ticketing standard | |
| gates | `scripts/gates/` | exit 0 or say why not | be skipped |

Subagents cannot spawn subagents, so only the orchestrator dispatches.

## Gates

`scripts/gates/run-all.sh [<issue>]` runs them in order and stops at the first
failure, printing `GATE / COMMAND / EXIT / OUTPUT`. `--only <id>` runs one.
"This change" means `$GATE_BASE...HEAD`: `origin/inherit` on a ticket branch,
`origin/v0.1-resolver` on `inherit` itself (PR #1's own diff).

| Gate | Script | Proves |
|---|---|---|
| G0 | `ticket-ready.sh <n>` | the issue is open, has `## Done when` and a fenced `## Verify`, is not blocked, needs-human, or split |
| G1 | `build.sh` | workspace builds, all targets; `trellis-cli --no-default-features` builds |
| G2 | `test.sh` | full workspace suite, and the local-only CLI suite |
| G2b | `guards.sh` | `qa/run-guards.py` holds every guard against the fresh debug binary; needs Ollama at 127.0.0.1:11434 |
| G3 | `lint.sh` | `cargo fmt --check`, `clippy -D warnings`, and no `#[allow(` added by this change |
| G4 | `no-stubs.sh` | no `TODO`/`FIXME`/`todo!`/`unimplemented!`/mock markers in production lines this change added (test modules, `tests/`, `examples/` excluded) |
| G5 | `security.sh` | `cargo deny check`, `cargo audit`, no key-shaped literal added |
| G6 | `ticket-verify.sh <n>` | the ticket's own Verify command exits 0 |
| G7 | in `run-all.sh` | exit 3 when the ticket is `risk:high` or `surface:public`: a person approves the merge |

Every gate unsets provider keys before it runs anything (ADR 0024 §3).
G6 runs text from an issue body, so each line must start with `cargo`, `git`,
`scripts/`, `py -3 qa/` or `python3 qa/`, and may not use `$(`, backticks,
pipes, `;`, `&` or redirection. Anything more belongs in a script under
`scripts/`, where it is reviewed as code.

A gate that cannot run fails. There is no skip variable and no `--force`; a
wrong gate is fixed in its own ticket.

### State at introduction

Run on `inherit` (so against PR #1's whole diff) at the commit that added
this file; the output is in the commit message.

- G1, G2, G2b (11/11 guards), G4, G5 pass.
- G3 fails on three `#[allow]` lines PR #1 added (issue #13). Ticket branches
  are measured against `inherit`, so this does not block them.
- G5 first failed on a key-shaped test canary in
  `crates/trellis-cli/tests/hosted_keys.rs`; that commit builds it at run time.
- G0 fails on every existing ticket until the architect adds `## Done when`
  and `## Verify`; that is the orchestrator's first job.

## The loop

1. `scripts/tickets/next.sh` → the next ready ticket (`--why` explains every
   exclusion).
2. G0. Failing → architect makes it workable → G0 again.
3. Branch `t<n>-<slug>` off `inherit`; dispatch per the routing table.
4. `run-all.sh <n>`. A failure becomes a fix ticket quoting the output; three
   failures on one gate stops the run.
5. Green (and G7 approved) → `git merge --no-ff` into `inherit`, push, close the
   issue with the gate summary.

`v0.1-resolver` moves only when the owner merges PR #1; the local pre-push hook
refuses any push that would advance it past `origin/inherit`.

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
