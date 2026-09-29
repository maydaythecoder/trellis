# ADR 0040 — The ticketing tool is a separate `trellis-tickets` binary

**Status:** Accepted.
**Date:** 2026-09-29
**Issue:** #75 (story S13). The crate and its commands: #76, #77, #78, #79,
#80, #81.
**Holds:** CLAUDE.md's rule that brief 4.12 fixes the `trellis` CLI surface
and that growth beyond it is owner-directed and recorded in an ADR; ADR 0024
§3 and CLAUDE.md's invariant that every child process drops the provider-key
variables; CLAUDE.md's invariant that every `git` spawn goes through
`trellis_vcs`.

## Context

trellis's own GitHub issues follow a ticketing standard: six label
dimensions (`area:`, `kind:`, `risk:`, `surface:`, `effort:`, `origin:`),
`!`-prefixed flags, a `triage:` state, and a generated block between
`<!-- triage:start -->` and `<!-- triage:end -->` whose YAML carries
`story`, `labels`, `flags`, `triage`, `triage_reasons`, `ready`,
`ready_failures`, `blocked_by`, `triaged_at` and `triage_version`. The
issues in S13 carry `triage_version: 2`.

What enforces it today is shell and Python under `scripts/`:

- `scripts/gates/ticket-ready.sh` (G0) decides whether one issue is
  workable: open; a `## Done when` section; a fenced command under
  `## Verify`; not `parked`, `triage:needs-human` or `split`; a
  `triage:blocked` issue has a `blocked_by`; every `blocked_by` entry is an
  issue number and is closed.
- `scripts/tickets/next.sh` prints the next ticket an agent may start, and
  with `--why` every open issue and why it is or is not next. It applies G0's
  rules and more: it requires `triage:auto` (or `triage:blocked` with every
  `blocked_by` closed) and excludes `type:story`, `risk:high`,
  `surface:public`, `area:checks` and `!security`, which a person decides
  (`docs/BUILD_SYSTEM.md`, "Where a person is needed").
- `scripts/tickets/sync-ready.py` reads `next.sh --why` on stdin and makes
  the `ready` label and the triage block's `ready:` and `ready_failures:`
  lines match it, writing only where they differ.

Nothing classifies an issue into the six dimensions, files one, finds label
drift across the open issues, or emits the triage as data. S13 asks for a
tool that does, with six commands. The issues first named it
`trellis ticket <...>`, a subcommand of the product binary. That grows the
surface brief 4.12 fixes, which is why this is an ADR. The owner decided on
2026-09-28 that it ships as a separate binary instead.

## Decision

1. **A separate crate and binary.** `crates/trellis-tickets` is a workspace
   member (the workspace already globs `crates/*`) with a `[[bin]]` named
   `trellis-tickets`, run as `cargo run -p trellis-tickets -- <command>`. It
   is not a subcommand of `trellis`, so the product CLI does not grow. The
   tool serves the people and agents who build trellis, not the users of a
   harness for Python repositories; CLAUDE.md already keeps maintainer
   tooling out of the CLI surface ("examples, not CLI surface").

2. **Its only first-party dependency is `trellis-core`.** It does not depend
   on `trellis-model`, `trellis-loop`, `trellis-vcs` or any other trellis
   crate, and no trellis crate depends on it. Third-party dependencies come
   from what `[workspace.dependencies]` already lists (for example `clap`,
   `serde`, `serde_json`, `thiserror`, `walkdir`). **No HTTP or GraphQL crate
   is added**, for this tool or for anything else it needs from GitHub.

3. **The rule engine is the library; the six commands are thin wrappers.**
   The library holds the label schema and one pure function per dimension
   (`area`, `kind`, `risk`, `surface`, `effort`, `origin`, `flags`, `ready`,
   `triage`) over a `Ticket { title, body, labels, state }`, with no I/O. A
   rule that cannot decide returns a value that says so, not an error:
   `Outcome<T>` with `Decided { value, rule }` and
   `Abstained { rule, reason }`. `TRIAGE_VERSION = 2`. The standard's text is
   committed as `docs/TICKETING.md` before any rule is written, so each rule
   has a written source to be checked against (#76). Tests for the rules live
   in this crate and need neither `gh` nor a network.

4. **The commands and their arguments.** These six, and no others without an
   amendment to this ADR:

   | Command | Arguments | Behaviour |
   |---|---|---|
   | `new` | `--title <t> --body-file <p> [--dry-run]` | Refuses a body without `## Done when` or a fenced `## Verify` (what G0 and G6 read), runs G6's line checks on the Verify block, and files the issue with the labels `triage` computes. `--dry-run` prints the `gh issue create` argv without running it. (#79) |
   | `triage` | `<n> [--json]` | One line per dimension with the rule that set it or the abstention's reason; `--json` emits one object. **Read-only**: never writes to GitHub. (#77) |
   | `ready` | `<n>` | G0's conditions verbatim, plus four named checks: `no verify command`, `decision words`, `effort:l`, `needs ADR`. Exit 0 when ready; otherwise exit 1 and every failure by name. (#78) |
   | `sweep` | `[--dry-run\|--apply]` | Re-triages every open issue and prints drift as `#n: -label +label (rule: ...)`. **Dry-run unless `--apply`**, and never touches an issue labelled `parked` or `split`. The only command that changes an existing issue. (#80) |
   | `scan` | `[<path>]` | Finds G4's markers (the list in `scripts/gates/no-stubs.sh`), with G4's exclusions (test modules, `tests/`, `examples/`), and prints `file:line` proposals for `origin:code` tickets. **Never files anything.** (#80) |
   | `index` | `[--out <p>]` | JSON with top-level `"triage_version": 2`, sorted by story then issue number; the schema is documented in `docs/TICKETING.md`. (#81) |

   Of the six, only `new` (filing) and `sweep --apply` (labels and the
   triage block) write to GitHub, and `new --dry-run` does not.

5. **GitHub is reached only by spawning `gh`.** Every spawn goes through one
   helper in the crate that applies `trellis_core::secrets::scrub` to the
   `Command` before it runs, so `gh` never inherits a provider key. This is
   enforced, not advisory:
   `crates/trellis-cli/tests/child_env.rs::every_process_spawn_in_the_workspace_removes_the_key_variables`
   scans every crate's `src/` and fails on a `Command::new` with no scrub
   beside it. Tests replace `gh` with recorded issue JSON or a stub, and
   never spawn it.

6. **`scan` does not spawn `git`.** The crate cannot reach `trellis_vcs`
   (decision 2), and `crates/trellis-cli/tests/hygiene.rs::every_git_spawn_goes_through_trellis_vcs`
   scans every crate. `scan` walks the files under `<path>` directly. This is
   also why it reads the whole tree rather than G4's "lines this change
   added": G4 needs a diff, and `scan` answers a different question.

7. **The scripts stay until the tool reproduces them.**
   - `scripts/gates/ticket-ready.sh` stays G0 until a script under
     `scripts/checks/` shows `trellis-tickets ready` and G0 agree on every
     open issue. `ready` is stricter than G0 by its four named extras, so the
     comparison is over G0's conditions; an issue that fails only an extra is
     reported by name beside the comparison, not counted as agreement or
     hidden.
   - `scripts/tickets/sync-ready.py` is replaced only once `sweep --apply`
     reproduces its writes (the `ready` label and the `ready:` and
     `ready_failures:` lines) on the current open issues.
   - `scripts/tickets/next.sh` is replaced only once the tool reproduces its
     output, both `next.sh` and `next.sh --why`, on the current open issues.
     None of the six commands prints the next ticket, and `next.sh` applies
     rules G0 does not (see Context). Reproducing it takes a command
     or argument this ADR does not list, so it is an amendment to this ADR,
     not something an implementing ticket adds. Until then `next.sh` stays
     what the agents obey.

## Alternatives rejected

- **`trellis ticket <...>` inside the product binary.** Grows the surface
  brief 4.12 fixes with a command no user of trellis needs, and puts a
  `gh`-spawning maintainer tool in the shipped binary. The same rules and
  commands would apply; only the home differs. Rejected by the owner,
  2026-09-28.
- **An HTTP or GraphQL client crate for GitHub.** A new dependency with TLS
  and token handling, where the token would be read and held by trellis code.
  `gh` already holds the user's GitHub credential and is already required by
  the gates (`scripts/gates/ticket-ready.sh` uses it). Spawning it keeps
  credential handling out of this repository.
- **Keep extending the shell scripts.** They cannot express an abstaining
  rule as a value or be tested without GitHub, and `next.sh` and G0 already
  hold one rule set in two places that must be kept in step by hand.
- **Replace the scripts when the crate lands.** An agent obeys `next.sh` and
  G0 today; swapping them before the tool is shown to agree would change
  which tickets agents start with no evidence it picks the same ones.

## Consequences

- The workspace gains one crate and one binary that is not `trellis`. The
  shipped `trellis` binary and its surface are unchanged.
- CLAUDE.md's Commands block and crate table gain `trellis-tickets` with the
  crate (#76), not before: a command listed there that does not build is the
  "appears to work" failure CLAUDE.md warns against.
- Until the parity conditions in decision 7 hold, there are two
  implementations of G0's rules, and the shell one is authoritative.
