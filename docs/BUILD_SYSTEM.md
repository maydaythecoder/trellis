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
| orchestrator | `.claude/agents/orchestrator.md` | picks tickets, dispatches, runs gates, merges into `audit` | write or edit files; push to `main` |
| architect | `.claude/agents/architect.md` | ADRs, `## Done when` / `## Verify` on tickets | write application code |
| implementer | `.claude/agents/implementer.md` | one ticket, on its branch, with tests | decide architecture; add deps, `#[allow]`, `unsafe` without an ADR |
| verifier | `.claude/agents/verifier.md` | runs gates, reports output verbatim | edit anything; give an opinion |
| security | `.claude/agents/security.md` | read-only review of trust-boundary changes | edit anything |
| planner | `.claude/agents/planner.md` | turns a request that is not yet a ticket into filed tickets and a dispatch order; fans out at most three agents, at most one running cargo | edit repository files |
| git-ci-steward | `.claude/agents/git-ci-steward.md` | ADR 0051's branch flow and promotion of `audit` into `dev`; CI, hooks, `deny.toml`, `scripts/gates`, `scripts/checks`, `scripts/tickets` | touch crate code; push to `main` or a `v*` tag |
| unparker | `.claude/agents/unparker.md` | ranks parked / needs-human tickets by what an answer unblocks, drafts one question each for the owner, applies the answers until a ticket needs nobody | decide for the owner; ask directly (the coordinator asks) |
| queue-runner | `.claude/agents/queue-runner.md` | works the agent-executable queue one ticket at a time: implement, gate, merge into `audit`, close | touch parked work; push to `main` or `v0.1-resolver` |
| queue | GitHub issues + Trellis roadmap project (#2) | the tickets, labelled per the ticketing standard | |
| gates | `scripts/gates/` | exit 0 or say why not | be skipped |

The orchestrator and the planner dispatch; an agent without the `Agent` tool
cannot. `.claude/settings.json` caps concurrent subagents at three, because the
host has 15 GB shared with a local model; at most one of them runs cargo.

## Gates

`scripts/gates/run-all.sh [<issue>]` runs them in order and stops at the first
failure, printing `GATE / COMMAND / EXIT / LOG / OUTPUT`. `--only <id>` runs one.
`OUTPUT` is the last 40 lines; `LOG` is the gate's full output, in
`$GATE_LOG_DIR` (default `${TMPDIR:-/tmp}/trellis-gates/<HEAD sha>/<gate>.log`,
and never inside the repository). A G2 failure also prints every
`---- <test> stdout ----` block and `panicked at` line from the full log (#178).
"This change" means `$GATE_BASE...HEAD`: `origin/dev` on a ticket, feature or
`audit` branch, `origin/main` otherwise (`dev`'s own PR diff); ADR 0051.

| Gate | Script | Proves |
|---|---|---|
| G0 | `ticket-ready.sh <n>` | the issue is open, has `## Done when` and a fenced `## Verify`, is not blocked, needs-human, or split |
| G1 | `build.sh` | workspace builds, all targets; `trellis-cli --no-default-features` builds |
| G2 | `test.sh` | full workspace suite, and the local-only CLI suite; each `cargo test` fails after `GATE_TEST_TIMEOUT_S` seconds (default 2400) rather than hanging (#173); tests run at most 4 at a time (`RUST_TEST_THREADS=4`) so the gate's own load does not fail timing tests (#226) |
| G2b | `guards.sh` | `qa/run-guards.py` holds every guard against the fresh debug binary, and fails unless `trellis` resolves to it; needs Ollama at 127.0.0.1:11434. Before #158, on Windows, it ran whatever `trellis` was installed instead |
| G3 | `lint.sh` | `cargo fmt --check`, `clippy -D warnings`, and no `#[allow(` added by this change |
| G4 | `no-stubs.sh` | no `TODO`/`FIXME`/`todo!`/`unimplemented!`/mock markers in production lines this change added (test modules, `tests/`, `examples/` excluded) |
| G5 | `security.sh` | `cargo deny --locked check` and `cargo audit` over the root and `fuzz/` locks; `fuzz/Cargo.lock` is current (`scripts/checks/fuzz-lock-current.sh`, #197); `zizmor --min-severity medium .github/workflows/` is clean and every workflow cargo build/test/clippy/run passes `--locked` (`scripts/checks/workflows-locked.sh`, #215); trellis-cli's test fixtures spawn no bare `git` (`scripts/checks/no-bare-git-in-cli-tests.sh`, #240/#241); gitleaks finds nothing in `$GATE_BASE..HEAD` and fails when absent, after a self-test that plants a token built at run time in a throwaway repository (`scripts/checks/gitleaks-range.sh`, #236; CI's supply-chain job runs it with a release pinned by version and SHA-256); no key-shaped literal added |
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

## Security: before every commit, and before every merge to main

Two layers (#206):

- **`scripts/hooks/pre-commit`**, installed into the shared git dir by
  `scripts/hooks/install.sh`, so it runs in the main checkout and in every
  worktree. It refuses a commit when:
  - its added lines carry a key-shaped literal or a basic-auth URL (G5's
    patterns, read from `scripts/checks/key-pattern.sh` and
    `no-auth-urls.sh`, so the two cannot drift);
  - it adds a secret-shaped file;
  - it edits the secret scanner's ignore files;
  - `gitleaks protect --staged` finds something;
  - it touches `.github/workflows/` and uses `pull_request_target`, puts a
    `${{ github.event.* }}` expression inside `run:`, or fails `actionlint`
    or `zizmor` at medium severity or above.

  Hits print redacted. `scripts/checks/pre-commit-selftest.sh` plants each
  case in a throwaway repository and shows the refusal fires. trellis's own
  git spawns disable hooks, so a run never executes it.
- **The `security-auditor` agent** (`.claude/agents/security-auditor.md`)
  runs the full audit: history, live credentials, GitHub settings, Actions,
  code and supply chain. It writes `security/AUDIT-<date>.md`. It runs before
  every merge of `dev` into `main`.

The workflows pass `zizmor` at medium and above since #215: every action is
pinned to a commit SHA resolved from its tag through the GitHub API (the tag
in a comment beside it), each workflow declares `permissions: contents:
read`, checkout does not persist the token, and release's rust-cache is
`lookup-only`. G5 runs zizmor over the whole directory, so a workflow change
that reached a branch without the hook is still held to it. To move a pin,
resolve the new tag with `gh api repos/<owner>/<repo>/git/ref/tags/<tag>`
(dereference an annotated tag with `.../git/tags/<sha>`) and never type a
SHA by hand.

## The loop

1. `scripts/tickets/next.sh` → the next ready ticket (`--why` explains every
   exclusion).
2. G0. Failing → architect makes it workable → G0 again.
3. Branch `t<n>-<slug>` off `dev`; dispatch per the routing table.
4. `run-all.sh <n>`. A failure becomes a fix ticket quoting the output; three
   failures on one gate stops the run.
5. Green (and G7 approved) → `git merge --no-ff` into `audit`, push, close the
   issue with the gate summary.

## Branches (ADR 0051)

| Branch | Role | Moved by |
| --- | --- | --- |
| `main` | launch: finished and ready to ship; the default branch; tagged per release | the owner, merging `dev`'s pull request on GitHub |
| `dev` | development trunk; what every ticket and feature branch is cut from | promotion from `audit` once its gates pass |
| `audit` | integration: ticket and feature branches meet here, conflicts are resolved, gates and the security audit run | `git merge --no-ff` of a green ticket or feature branch |
| `t<n>-<slug>`, `feature/<slug>` | one ticket, or a feature spanning several | their author; deleted after merging |

The legacy branches `v0.1-resolver` (the old default, frozen), `drain` (the
old integration branch, frozen at the commit `dev` and `audit` started from),
`inherit`, `agents-setup` and `orchestrator-state` are kept, not deleted.

The pre-push hook (`scripts/hooks/pre-push`,
installed by `scripts/hooks/install.sh`; `scripts/checks/pre-push-selftest.sh`
shows it firing) refuses every push to `main`, `v0.1-resolver` and the frozen `drain`, and of any `v*` tag (ADR 0051 item 3), from a clone that has
installed it (GitHub branch protection needs Pro on a private repository;
ADR 0036 records the gap and the ruleset to apply later).

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
- **Secret scanner**: gitleaks runs in G5 and in CI over the change's
  commits (#236); the key-shape regex over added lines stays beside it
  because the pre-commit hook shares its patterns. GitGuardian still runs on
  the PR.

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
  against it for public ones. The owner decided on 2026-10-04 to move both
  runners to low-privilege accounts (#219); the procedure is below. Until the
  owner has run it and one CI run is green on each runner, the table above is
  still the current state.

### Low-privilege runner accounts (#219)

Two scripts, both run by the owner with admin rights. Both print every step,
have a dry-run mode, refuse to start without admin rights, refuse while their
runner is running, and are safe to run again. Neither prints `.credentials`
or a token. Neither needs a new registration token, because each moves the
existing registration with the runner directory. On Windows that works
because the runner encrypts `.credentials_rsaparams` with DPAPI at
`LocalMachine` scope, not per user.

| | `trellis-wsl` | `trellis-win` |
|---|---|---|
| Script | `scripts/runners/wsl-runner-account.sh` (as root in the distro) | `scripts/runners/win-runner-account.ps1` (elevated) |
| Account | `trellis-ci`: own group, not in `sudo`/`adm`/`docker`/`ollama`, password locked, `nologin` shell | `trellis-ci-win`: member of `Users` only, hidden from sign-in, denied local, RDP and network logon, allowed batch logon |
| Runner dir | moved to `/home/trellis-ci/actions-runner`, owned by `trellis-ci`, mode 700, `.credentials*` 600 | stays at `C:\actions-runner\win`; owner `trellis-ci-win`; ACL is SYSTEM, Administrators and `trellis-ci-win` only (the owner reads it elevated, or rerun with `-OwnerCanRead`) |
| Started by | systemd unit `trellis-runner-wsl.service` (`User=trellis-ci`). The `trellis-runner-wsl` task becomes a keep-alive (`wsl.exe -d Ubuntu --exec /bin/sleep infinity`) as the owner at logon, because the distro stops when no `wsl.exe` is left | task `trellis-runner-win`, as `trellis-ci-win`, at startup, whether or not anyone is logged on, no time limit |
| Job PATH | `.path` loses every `/home/mayda` and `/mnt` entry; the runner's `cargo/bin` comes first | the account's own PATH, plus `%~dp0cargo\bin` added in `start-runner.cmd` |

Things the scripts close beyond the account change:
- **WSL interop.** Any Linux user can read `/mnt/c` (9p, mode 777), and can
  start a Windows `.exe` through `/init`. That `.exe` then runs with the
  owner's Windows token. So a non-sudo account alone would not contain a
  job. The unit makes `/mnt/c`, `/mnt/wslg`, `/run/WSL` and `/init`
  inaccessible (not all of `/mnt`: `/etc/resolv.conf` links to
  `/mnt/wsl/resolv.conf`, so hiding it breaks DNS), mounts an
  empty `/home` with only `/home/trellis-ci` bound in, and sets
  `NoNewPrivileges`, `PrivateTmp` and `ProtectSystem=full`.
- **`C:\` lets every signed-in user write to new top-level folders.** Its ACL
  gives `Authenticated Users` Modify on subfolders, so `C:\w` (the worktrees
  and their hooks) and `C:\ci-local` are writable by any account. The
  script adds a Deny ACE for `trellis-ci-win` on each `-DenyPaths` entry
  (default `C:\w`, `C:\ci-local`). It also lists any other top-level folder
  with the same ACL and leaves it unchanged.
- **WSL `.credentials` was world-readable** (mode 644). It is now 600.

Each script keeps the provider-key scrub. `start.sh` and `start-runner.cmd`
still clear `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GROQ_API_KEY` and
`GEMINI_API_KEY`, and the Windows script refuses to run if those lines are
missing. The runner's `.env` (its own `CARGO_HOME`, `RUSTUP_HOME`,
`GIT_CONFIG_GLOBAL` and a closed `OPENAI_BASE_URL`) is unchanged apart from
the WSL path. rustup is not copied: `dtolnay/rust-toolchain` installs it into
the runner's `CARGO_HOME` on the first job, and the PATH change lets later
jobs find it there. MSVC Build Tools, Git and the system `python3` are
installed per machine, so the new accounts can use them.

Run, in this order. Run the dry runs first and read their output.

```
# 1. Stop both runners. Wait for an idle runner, or a running job is cancelled.
schtasks /end /tn trellis-runner-wsl
schtasks /end /tn trellis-runner-win

# 2. WSL (from any prompt; -u root needs no password):
wsl.exe -d Ubuntu -u root -- bash scripts/runners/wsl-runner-account.sh --dry-run
wsl.exe -d Ubuntu -u root -- bash scripts/runners/wsl-runner-account.sh

# 3. Windows, from an elevated PowerShell in the repository:
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\runners\win-runner-account.ps1 -WhatIf
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\runners\win-runner-account.ps1
schtasks /run /tn trellis-runner-win
schtasks /run /tn trellis-runner-wsl
```

If a runner refuses because it is still running, wait, or add `-StopRunner` /
`--stop-runner`. Those options also cancel a job in progress.

Verify:
- `gh api repos/maydaythecoder/trellis/actions/runners --jq '.runners[] | [.name, .status, .busy] | @tsv'`
  lists `trellis-win` and `trellis-wsl` as `online`.
- Inside a job, `whoami` prints `muhyadins_pc\trellis-ci-win` on Windows and
  `trellis-ci` on Linux. For a one-off check, add a step `- run: whoami; id || true`
  on a scratch branch. The same check works without CI:
  `Get-CimInstance Win32_Process -Filter "Name='Runner.Listener.exe'" | Invoke-CimMethod -MethodName GetOwner`
  and `wsl.exe -d Ubuntu -- ps -o user=,args= -C Runner.Listener`.
- `wsl.exe -d Ubuntu -- systemctl status trellis-runner-wsl` shows the unit
  as active. `schtasks /query /tn trellis-runner-win /v /fo list` shows
  `Run As User: trellis-ci-win` and `Logon Mode: Interactive/Background`.
- Push a commit to a branch with CI. Both runners finish one green run.
- After a reboot, `trellis-win` comes back without a logon. `trellis-wsl`
  still waits for the owner's logon, which starts the WSL keep-alive.

Roll back, in this order. Both rollbacks keep the account and lock or
disable it. Each prints the command that deletes it.

```
wsl.exe -d Ubuntu -u root -- bash scripts/runners/wsl-runner-account.sh --rollback
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\runners\win-runner-account.ps1 -Rollback   # elevated
```

The WSL rollback removes the unit, moves the directory back to
`/home/mayda/actions-runner`, and restores `.env`, `start.sh`, `ci-gitconfig`
and `.path` from their `*.pre219` copies. The Windows rollback restores the
ACLs from `C:\actions-runner\win.acl-pre219.txt` and gives ownership back to
the owner. It removes the Deny ACEs, restores `start-runner.cmd.pre219`, and
re-registers both tasks as the owner at logon, as they were before #219.

This differs from the ticket's owner steps in two ways. The Windows runner
runs as a task under `trellis-ci-win`, not as a service, so that
`start-runner.cmd`, which does the scrub, stays the single launcher. And the
scripts move the existing registrations instead of creating new ones. If a
fresh registration is wanted anyway: run `config.cmd remove` / `./config.sh
remove` with a removal token, then `config --url
https://github.com/maydaythecoder/trellis --token <token> --name trellis-win
--labels self-hosted,Windows,X64 --unattended` as the new account, before
running the scripts.

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
