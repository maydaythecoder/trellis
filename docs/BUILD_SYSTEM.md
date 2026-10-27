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

Unattended, an hourly scheduled cloud session follows `docs/RUNNER.md`
instead: it sets itself up with `scripts/cloud-setup.sh`, works the queue
through the same gates, promotes `audit` into `dev`, and reports on #925.

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
| G1 | `build.sh` | workspace builds, all targets; `trellis-cli --no-default-features` builds. Go branch (#504): `go build ./...` in `tui/` |
| G2 | `test.sh` | full workspace suite, and the local-only CLI suite; each `cargo test` fails after `GATE_TEST_TIMEOUT_S` seconds (default 2400) rather than hanging (#173); tests run at most 4 at a time (`RUST_TEST_THREADS=4`) so the gate's own load does not fail timing tests (#226). Go branch (#504): `go test -count=1 ./...` in `tui/`, under the same time limit |
| G2b | `guards.sh` | `qa/run-guards.py` holds every guard against the fresh debug binary, and fails unless `trellis` resolves to it; needs a listener at 127.0.0.1:11434 (Ollama on the host; `scripts/cloud-setup.sh` starts a plain one in a cloud session, as `ci.yml` does). Before #158, on Windows, it ran whatever `trellis` was installed instead |
| G3 | `lint.sh` | `cargo fmt --check`, `clippy -D warnings`, and no `#[allow(` added by this change; ADR numbers unique and held, also against the base (`scripts/checks/adr-numbers.sh`, #284), and the exit-gate check's self-test (`gate-checked.sh`); CI runs both, the tree half in the `guards` job and the base half in `supply-chain` (#532). Go branch (#504): `gofmt -l` prints nothing, `go vet ./...`, and no `//nolint` added |
| G4 | `no-stubs.sh` | no `TODO`/`FIXME`/`todo!`/`unimplemented!`/mock markers in production lines this change added (test modules, `tests/`, `examples/` excluded). Go branch (#504): the same markers, and `panic("not `, in added non-test `*.go` lines under `tui/` |
| G5 | `security.sh` | `cargo deny --locked check` and `cargo audit` over the root, `fuzz/` and `desktop/` locks, the desktop's under `desktop/deny.toml` (ADR 0048 §7); `fuzz/Cargo.lock` is current (`scripts/checks/fuzz-lock-current.sh`, #197); Tauri stays out of the root lock, `desktop/Cargo.lock` is current, `desktop/deny.toml` is the root policy plus its `desktop-only` lines, no `audit.toml` exists, and the desktop's code stays inside ADR 0048's lines (`scripts/checks/desktop-guards.sh`, after its self-test, #290); `zizmor --min-severity medium .github/workflows/` is clean and every workflow cargo build/test/clippy/run passes `--locked` (`scripts/checks/workflows-locked.sh`, #215); trellis-cli's test fixtures spawn no bare `git` (`scripts/checks/no-bare-git-in-cli-tests.sh`, #240/#241); gitleaks finds nothing in `$GATE_BASE..HEAD` and fails when absent, after a self-test that plants a token built at run time in a throwaway repository (`scripts/checks/gitleaks-range.sh`, #236; CI's supply-chain job runs it with a release pinned by version and SHA-256); no CI job dropped (`ci-jobs-unchanged.sh`) and the pre-push, pre-commit and ci-local self-tests pass; no key-shaped literal or basic-auth URL added (`key-pattern.sh`, `no-auth-urls.sh`). CI's `supply-chain` job runs the cargo deny and cargo audit lines, the fuzz and desktop checks, `workflows-locked.sh`, gitleaks and `ci-jobs-unchanged.sh`; its `guards` job runs `no-auth-urls.sh` over the whole tracked tree, the bare-git check and the pre-push and pre-commit self-tests (#532), and `zizmor --min-severity medium .github/workflows/` from a hash-pinned install (`.github/requirements-zizmor.txt`, #534). Not in CI: the ci-local self-test (it needs PyYAML, which CI's hashed `qa/requirements-ci.txt` does not pin) and the key-shape scan (G5 applies it to added lines only; gitleaks covers the change's commits in CI). Go branch (#504, ADR 0052): `scripts/checks/go-guards.sh --self-test` and then `go-guards.sh` (no `exec` outside `tui/internal/rpc/`, no `git` spawn, no environment change, no `replace` and a `toolchain` line in `tui/go.mod`); then `go mod verify`, `govulncheck ./...` and `go-licenses check` with `deny.toml`'s licence list, each tool absent failing the gate; `workflows-locked.sh` also holds every workflow `go build/test/vet/run/install` to `-mod=readonly` and every `go install` to an exact `@vX.Y.Z` |
| G6 | `ticket-verify.sh <n>` | the ticket's own Verify command exits 0 |
| G7 | in `run-all.sh` | exit 3 when the ticket is `risk:high` or `surface:public`: a person approves the merge |

Every gate unsets provider keys before it runs anything (ADR 0024 §3).
Each Go branch runs only when `tui/go.mod` exists, and its gate's OK line
says it skipped otherwise. With `tui/` present, a missing `go` fails the
gate, and every go command runs with `GOFLAGS=-mod=readonly` and
`GOTOOLCHAIN=local` (ADR 0052 decision 2.4).
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
- **`.github/workflows/audit.yml`**, a weekly scheduled run (Mondays, 08:17
  UTC, and `workflow_dispatch`) of the same `cargo deny --locked check` and
  `cargo audit` the `supply-chain` job runs, against `main`. New advisories
  land against unchanged code, and nothing else re-checks a tree that sits
  still (#213 T6, #220). Decided 2026-10-04 for the self-hosted runners
  after #219; recorded 2026-10-23 on a GitHub-hosted runner instead, since
  those runners are set aside and a hosted runner holds none of the owner's
  credentials. Same pins as `ci.yml`. GitHub runs a schedule only from the
  file on `main`, so the first scheduled run follows the next `dev` → `main`
  promotion.

The workflows pass `zizmor` at medium and above since #215: every action is
pinned to a commit SHA resolved from its tag through the GitHub API (the tag
in a comment beside it), each workflow declares `permissions: contents:
read`, checkout does not persist the token, and release's rust-cache is
`lookup-only`. G5 runs zizmor over the whole directory, so a workflow change
that reached a branch without the hook is still held to it, and CI's
`guards` job runs the same command before its build (#534), so one that
skipped both is too. CI installs zizmor from
`.github/requirements-zizmor.txt` with `pip install --require-hashes`: one
version, a hash for every file PyPI has for it, the regeneration command in
the file's header. Move it to the version the hook and G5 run. To move a pin,
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

These are the only permanent branches; the legacy ones were deleted on
2026-10-06, and ADR 0051 item 5 records their tips.

`audit` reaches `dev` through an `audit → dev` pull request, and `dev`
reaches `main` through a `dev → main` one. CI (`ci.yml`) runs on each of
those pull requests and on a push to `main`, not on a push to `dev`: the
`audit → dev` pull request has already run it on the tree `dev` receives,
and a cancelled duplicate run on the `dev` push showed as failed checks on
the `dev → main` pull request (#473; ADR 0051 item 4, amended by #477).

The pre-push hook (`scripts/hooks/pre-push`,
installed by `scripts/hooks/install.sh`; `scripts/checks/pre-push-selftest.sh`
shows it firing) refuses every push to `main`, and to the deleted `v0.1-resolver` and `drain` so an old clone cannot re-create them, and of any `v*` tag (ADR 0051 item 3), from a clone that has
installed it (GitHub branch protection needs Pro on a private repository;
ADR 0036 records the gap and the ruleset to apply later).

### Versions and releases (#386)

The version is `[workspace.package].version` in `Cargo.toml`, which every
crate inherits, and `version` in `desktop/Cargo.toml`; the two stay equal.
`CHANGELOG.md` records what each version holds. A release:

1. **Bump on `dev`**, through a ticket branch and `audit` like any change:
   set both versions to `X.Y.Z`; refresh the three locks with
   `cargo update -w --offline`, `cargo update -w --offline --manifest-path
   desktop/Cargo.toml` and `cargo metadata --format-version 1 --offline
   --manifest-path fuzz/Cargo.toml` (each changes only the trellis packages'
   `version` lines; a diff with anything else is wrong); move
   `CHANGELOG.md`'s `## [Unreleased]` entries under a new `## [X.Y.Z]`.
2. **The owner merges** the `dev → main` pull request.
3. **The owner tags `vX.Y.Z` on `main`** and pushes the tag. The pre-push
   hook refuses `v*` tags on purpose (ADR 0051 item 3): the push is the
   owner's act, not an agent's.
4. `release.yml` builds and tests Linux (`x86_64-unknown-linux-gnu`) and
   Windows (`x86_64-pc-windows-msvc`) on GitHub-hosted runners
   (`ubuntu-latest`, `windows-latest`; as of 2026-10-19, see "CI runners"),
   and nothing else: macOS is dropped until a macOS runner exists (owner
   decision 2026-10-06, ADR 0052 decision 2.5; #506). Each leg ships
   `trellis` and the Go TUI `trellis-tui` side by side, `trellis-tui` built
   from `tui/` with the pinned Go that `scripts/runners/go-hosted.sh`
   unpacks for the job (`CGO_ENABLED=0`, `-trimpath`,
   `-buildvcs=false`, `-ldflags "-X main.version=X.Y.Z"`, or `dev` from
   `workflow_dispatch`), and its `SHA256SUMS` lists both. Its `publish`
   job, on the Linux runner, which needs `gh` and `zip`, refuses unless
   the tag is `vX.Y.Z`, its commit is an ancestor of `origin/main`, and
   the Linux `trellis --version` is `trellis X.Y.Z` and `trellis-tui
   --version` is `trellis-tui X.Y.Z`. Only then does it create the GitHub
   release with one archive per target and a `SHA256SUMS` over them.
   `publish` is the only job in any workflow with `contents: write`.

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
  because the pre-commit hook shares its patterns. CI's `guards` job also
  runs `scripts/checks/no-auth-urls.sh` over the whole tracked tree, so a
  basic-auth URL literal committed with `--no-verify` still fails (#532).
  GitGuardian still runs on the PR.

## CI runners

As of 2026-10-19 (#736), every job in `ci.yml` and `release.yml` runs on
GitHub-hosted runners: `ubuntu-latest` and `windows-latest`, named directly
in each `runs-on`. The `go` and `supply-chain` jobs and the release build
get the pinned Go from `scripts/runners/go-hosted.sh`, which unpacks the same hash-checked
go.dev archive that `install-go.sh` records for a self-hosted runner, into
`$RUNNER_TEMP` only. The `desktop` job installs its Tauri prerequisites
with apt on the hosted Linux image, which does not ship WebKitGTK.
`rust-cache` runs on these runners. CI runs on pull requests into `dev` and
`main` and on a push to `main`; nothing runs on a pull request into `audit`
(the `on:` comment in `ci.yml` says why).

The self-hosted runners below are set aside as of 2026-10-19: the runner
host stopped taking jobs on 2026-10-12, every run queued until the next push
cancelled it, and #736 moved the jobs off them so promotion PRs could merge
again. The table and the procedure that follow are kept as the record of
that setup; the low-privilege account move is #219, open and parked. What
was true until then: GitHub-hosted jobs stopped starting on 2026-09-28
("recent account payments have failed or your spending limit needs to be
increased") and CI ran on two self-hosted runners on the owner's machine
instead; a smoke job on one ran while GitHub-hosted jobs were blocked (run
36507327810).

| Runner (set aside 2026-10-19, see #219) | Where | Labels | Started by |
|---|---|---|---|
| `trellis-win` | `C:\actions-runner\win` (Windows host) | self-hosted, Windows, X64 | `start-runner.cmd` there |
| `trellis-wsl` | `~/actions-runner` in WSL2 Ubuntu 24.04 | self-hosted, Linux, X64 | `wsl.exe -d Ubuntu -- ~/actions-runner/start.sh` |

- Until #736, each `runs-on` in `ci.yml` read `vars.LINUX_RUNNER` /
  `vars.WINDOWS_RUNNER` (repository variables holding a JSON label list) and
  fell back to `ubuntu-latest` / `windows-latest` when unset. `ci.yml` and
  `release.yml` no longer read those variables, and since 2026-10-23 (#248)
  neither does `mutants.yml`, so bringing a self-hosted runner back to any of
  them is a workflow change, not a variable.
- Each runner's `.env` gives jobs their own `CARGO_HOME`, `RUSTUP_HOME` and
  `GIT_CONFIG_GLOBAL` under the runner's directory. Without them a job runs as
  the owner with the owner's homes: on 2026-09-28 `Swatinem/rust-cache`'s
  post-job cleanup of `$CARGO_HOME/bin` deleted rustup and every installed
  cargo tool on both machines, and `ci.yml`'s `git config --global` steps
  would have rewritten the owner's git config (#158). rust-cache now runs on
  GitHub-hosted runners only. Each launcher strips the provider-key variables.
- The runners are processes, not services: they stop on logout or reboot and
  are restarted with the commands above. One job runs per runner at a time.
- WSL ran its own Ollama on 127.0.0.1:11434, so the `guards` job's stand-in
  listener did not bind there; the job only needs something listening on
  that port, and on a hosted runner the stand-in binds.
- Self-hosted runners run whatever a pushed workflow says, as the owner. That
  is acceptable for a private repository with one owner; GitHub advises
  against it for public ones. The owner decided on 2026-10-04 to move both
  runners to low-privilege accounts (#219); the procedure is below. It was
  not run before the runners were set aside, so if they return it is still
  the step before a job runs on them.

### Low-privilege runner accounts (#219)

Parked with the self-hosted runners (2026-10-19). The procedure below is the
one to run if they return.

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

### Go on the runners (#503)

The self-hosted procedure; set aside with the runners on 2026-10-19. On a
GitHub-hosted runner `scripts/runners/go-hosted.sh` unpacks the same pinned
archive per job ("CI runners" above); the pin and the hashes stay in
`install-go.sh`, read by both.

`tui/` is a Go module (ADR 0052). Each runner gets its own pinned Go from
`scripts/runners/install-go.sh`, never the owner's install (`C:\Program
Files\Go` on Windows is the owner's, not a pin) and never `actions/setup-go`
(refused by `scripts/ci-local.py`).

- **Pin:** Go 1.27.2. The script holds the version and the go.dev SHA-256 of
  `go1.27.2.linux-amd64.tar.gz` and `go1.27.2.windows-amd64.zip`. It
  refuses to unpack an archive whose hash differs, before anything is
  written. Once they exist (#505), `tui/go.mod`'s `toolchain` line and CI's
  `go version` check name the same pin.
- **Where:** under the runner's own directory only. The script refuses a
  target outside it, by path, `..` or symlink, a directory without a
  `.runner` file, and a system path. GOROOT is
  `<runner>/toolchains/go1.27.2/go`.
- **Variables**, written into the runner's `.env`: `GOROOT`, `GOPATH`
  (`<runner>/go-path`), `GOMODCACHE` (`<runner>/go-path/pkg/mod`),
  `GOCACHE` (`<runner>/go-cache`), `GOTOOLCHAIN=local` (a `toolchain` line
  in `go.mod` never downloads another toolchain) and `GOFLAGS=-mod=readonly`
  (a build never rewrites `go.mod` or `go.sum`). The pinned Go's `bin` goes
  first in `.path`. On Windows both files get Windows paths.
- **Run** once per runner, after #219, with the runner stopped, then restart
  it. Run again with the same pin, it changes nothing.

```
# WSL, from cmd or Git Bash (PowerShell has no `<`): as trellis-ci, which
# owns the runner directory (mode 700). The script
# is read from stdin because trellis-ci cannot read the owner's checkout.
wsl.exe -d Ubuntu -u root -- sudo -u trellis-ci -H bash -s -- --runner-dir /home/trellis-ci/actions-runner --dry-run < scripts/runners/install-go.sh
wsl.exe -d Ubuntu -u root -- sudo -u trellis-ci -H bash -s -- --runner-dir /home/trellis-ci/actions-runner < scripts/runners/install-go.sh
# Windows: from an elevated Git Bash. trellis-ci-win is denied interactive
# logon; the runner directory's ACL admits Administrators, and what the
# script writes there inherits trellis-ci-win's access.
bash scripts/runners/install-go.sh --runner-dir /c/actions-runner/win --dry-run
bash scripts/runners/install-go.sh --runner-dir /c/actions-runner/win
```

`--runner-dir <dir>` names another runner directory; `--self-test` shows
that a tampered archive and an outside target are refused.

**Moving the pin** is one commit that changes `GO_VERSION` and both hashes
in `install-go.sh`, copied from `https://go.dev/dl/?mode=json`, together
with `tui/go.mod`'s `toolchain` line and CI's `go version` check. The owner
then runs the script again on both runners; the old toolchain's `bin` is
dropped from `.path`, and its directory can be deleted by hand.

### Mutation testing on CI (#248)

`.github/workflows/mutants.yml` runs cargo-mutants with the whole workspace
suite against each mutant. These are the flags `WORKSPACE_TESTS=1
scripts/mutate-by-crate.sh` uses: `-p <crate> --test-workspace true
--timeout 300 -j 1`. It runs on `workflow_dispatch` only, never on a push or
a pull request. It runs on GitHub-hosted runners (`ubuntu-latest`,
`windows-latest`; it read `vars.LINUX_RUNNER` / `vars.WINDOWS_RUNNER` until
2026-10-23, see "CI runners") and uses the same pins as `ci.yml`: action SHAs, toolchain 1.93.0, cargo-mutants 27.1.0 installed with
`--locked`, and `--locked` on every cargo command it runs. It uploads
`mutants.out/` as the artifact `mutants-<crate>-<os>-<commit>[-shard-k-of-n][-file-<stem>]`,
kept for 30 days. The job limit is 360 minutes, GitHub's limit for hosted
runners. A shard that reaches the step's 345-minute limit still uploads the
outcomes it has.

GitHub dispatches a workflow by name only when the file is on the default
branch (`main`), so the hosted `runs-on` takes effect once the change is
promoted to `main`. `--ref` picks the branch whose head is tested.

```
gh workflow run mutants.yml -f crate=trellis-cli                      # whole crate, one job
gh workflow run mutants.yml --ref dev -f crate=trellis-cli -f shard=0/10
gh workflow run mutants.yml -f crate=trellis-cli -f os=windows -f shard=0/10
gh workflow run mutants.yml -f crate=trellis-cli -f os=linux -f file=<path>   # one file's mutants, unsharded
gh run list --workflow mutants.yml
gh run download <run-id>                                              # mutants-…/caught.txt, missed.txt, timeout.txt, unviable.txt
```

trellis-cli has about 1,500 viable mutants, and each one runs the full suite,
which takes about 46 seconds warm, plus each mutant's build. One job cannot run them
all in 6 hours. Dispatch shard `0/10` first and read its wall time, then size
`n` for the other shards. Every shard of one run must use the same `n`, or the
shards do not split the mutants between them. Dispatch the next shard after
the previous one finishes. (Until 2026-10-23 the workflow read the runner
variables; with the self-hosted runners set aside, a dispatch queued on an
offline runner and was cancelled after 24 hours. It now names the hosted
runners directly.)

The optional `file` input (#556) passes `-f <path>` to cargo-mutants, so only
that source file's mutants run, e.g. `-f file=crates/trellis-cli/src/main.rs`.
Use it to check one change, the kills a ticket claims for one file, or one
file's survivors, without a crate-wide sharded run: one file's mutants fit in
one job, so leave `shard` empty. The path must be a relative path that exists
at the dispatched commit, with no `..`; the inputs step refuses anything else.

`os=linux` (the default) reaches the `#[cfg(unix)]` code. The 22 `not
examined (#248)` lines need it. `os=windows` is the host #248's survivor
list came from. cargo-mutants mutates source text whatever its `cfg`, so a
mutant in code one leg does not compile is reported missed on that leg by
construction. That is how the 22 unix-only lines survived on Windows. A
mutant is confirmed caught when the leg that compiles its code catches it.

To check #248's kills, compare each mutant by its function and mutation
text, ignoring `file:line:col`, as the ledger's trellis-cli comparison does.
The main.rs split moved code between files, so line numbers no longer match.
Concatenate the shards' `missed.txt` first. Then:

```
strip() { sed -E 's/^.*crates\/trellis-cli\/src\/[^:]+:[0-9]+:[0-9]+: //' | sort -u; }
grep -E '^killed \(#248\)' CORRECTNESS_LEDGER.md | strip > ledger-killed.txt
grep -E '^(not examined|unix-only) \(#248\)' CORRECTNESS_LEDGER.md | strip > ledger-unix.txt
cat */missed.txt | strip > missed-now.txt
comm -12 ledger-killed.txt missed-now.txt      # a kill the run does not confirm
comm -12 ledger-unix.txt missed-now.txt        # Linux leg: a unix-only mutant still alive
```

The 352 `killed` lines strip to 300 distinct texts, because the same
mutation recurs at different lines of one function. A match means "look at
that line", not "refuted". Read the full lines in `missed.txt` before you
decide. Run the first `comm` once per leg. A line that appears in one leg's
list and in the other leg's `caught.txt` is code the first leg does not
compile (`#[cfg(windows)]` on Linux, `#[cfg(unix)]` on Windows). It is not an
unconfirmed kill. A line that appears in both legs' lists is an unconfirmed
kill. The kills are confirmed when there are none. The second
`comm` applies to the Linux leg only, since those 22 mutants survive on
Windows by construction. On Linux it should list only the 5 `probe_ps`
mutants. That code runs only on macOS and BSD, so this workflow cannot reach
it.
Check `timeout.txt` too. A timeout is neither caught nor missed, so a ledger
kill that shows up there is unconfirmed, not refuted. The script's rule that
more than two timeouts makes a run unusable applies to a mutation score. It
does not apply to this check, which reads one mutant at a time. trellis-cli
always has a dozen mutants that make a loop never end. A line marked
`equivalent` or `unexercisable` is expected in `missed.txt`. Record the run in
the ledger's #248 paragraph: its commit, the shards and the counts. #248
closes when the run confirms the kills (owner, 2026-10-06).

## CI without GitHub

`py -3 scripts/ci-local.py [--ref <commit>] [--job <name>]... [--keep-going]`
runs every job in `.github/workflows/ci.yml` on this machine: Linux legs in
WSL, Windows legs natively, one at a time, each in a clean checkout of the
commit under `C:\ci-local` (or `~/ci-local` in WSL). `uses:` steps are
replaced (checkout, toolchain) or skipped (rust-cache); setup-python becomes a
virtualenv; an action it does not know fails the job. Jobs get a throwaway
`GIT_CONFIG_GLOBAL` and `CARGO_INSTALL_ROOT`, no provider keys, and a closed
`OPENAI_BASE_URL`. Exit 0 only when every leg passed. It posts nothing to
GitHub; CI's own runners do that.
