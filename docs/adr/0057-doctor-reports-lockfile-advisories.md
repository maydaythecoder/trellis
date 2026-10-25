# ADR 0057 — `trellis doctor` reports advisories and yanked crates in the repository's `Cargo.lock`

**Status:** Accepted (owner-directed). Built.
**Date:** 2026-10-25
**Issue:** #779.
**Decided by:** the owner, 2026-10-25, in the session: `doctor` reports
lockfile advisories, naming the crate and the `cargo update -p <crate>` that
fixes it; it does not write the lock.
**Holds:** ADR 0024 §3 (every child process drops the provider-key
variables); CLAUDE.md "Dependency pinning" (a lockfile update is its own
commit). The CLI surface (brief 4.12) is unchanged: this is a line in an
existing command, not a flag or a command.

## Context

CI on `main` at b49920e failed on 2026-10-24 (run 40056551866) in the
`supply-chain` job, on two findings no commit caused: tokio 1.53.1
(RUSTSEC-2026-0386, published after the lock was written) and clipboard-win
5.4.1 (yanked after it). `dev` already carried both fixes (#770, #775), so
`main` was red only because its lock had aged against the RustSec database.

Nothing in trellis said so before CI did. `doctor` reports the model, the
runtime tier, the checkers, the index, the worktree, the skills, the control
dial, the budget, the sandbox and the trusted inputs, and nothing about the
lock a release is built from. The release that `main` is (ADR 0052 §7) is
the one a person would build on their own machine, where `doctor` is what
they run first.

## Decision

1. **A `lockfile:` line in `trellis doctor`**, after `worktree:`, printed
   only when `<root>/Cargo.lock` exists. A Python repository, which is what
   trellis is pointed at, has no lock and gets no line: a key that always
   reads "not applicable" teaches the reader to skip the column.

2. **Read from cargo-audit**, spawned as `cargo-audit audit --json --file
   <root>/Cargo.lock`: the binary `cargo audit` dispatches to, looked up on
   `PATH` by trellis itself and spawned by that path, so "not on PATH"
   names the tool and not cargo, and means this process's `PATH` alone
   (std's Windows resolver would also search the parent's `PATH` and the
   system directories). The provider keys are scrubbed from its
   environment (ADR 0024 §3). It is bounded at 90 seconds and killed at the
   bound, and its pipes are read under the same bound: its network work is
   a fetch of rustsec/advisory-db through a git library and a refresh of
   the crates.io index for the yanked check, and `doctor` must still end on
   a machine with no route to either.

3. **What counts as a finding is what `deny.toml` fails on**, not what
   cargo-audit's exit code does. cargo-audit exits non-zero only on a
   vulnerability and reports `unsound`, `unmaintained`, `notice` and
   `yanked` as warnings; CI's first step, `cargo deny --locked check`,
   fails on every advisory not listed in `[advisories].ignore`, and that is
   what turned `main` red (RUSTSEC-2026-0386 is `unsound`, not a
   vulnerability). So every entry in the report, `vulnerabilities.list[]`
   and each `warnings.<kind>[]`, is a finding unless its advisory id is in
   `deny.toml`'s ignore list, read from the root in either form cargo-deny
   takes (a string, or a table with `id`). A yanked version has no id and
   is always a finding. The ignored ones are counted on the line, not
   named: RUSTSEC-2023-0089 is the case, and the reason it is accepted is
   in `deny.toml` beside it. No `deny.toml`, or one that does not parse,
   ignores nothing: the line reports more, never less.

4. **The three answers.**
   - Clean: `lockfile:   Cargo.lock clean — cargo-audit against the RustSec
     database of <date>`. The date is the database's `last-updated`, because
     a clean verdict is only as current as the database it was read from,
     and the whole reason the line exists is that a lock goes stale by time.
   - Findings: each crate with its locked version and, in parentheses, the
     advisory id and kind (`tokio 1.53.1 (RUSTSEC-2026-0386, unsound)`) or
     `yanked`; the count of distinct advisories ignored; then the one
     command that fixes them, `cargo update -p <crate>@<version>` once per
     locked version (a bare name is ambiguous to cargo when the lock holds
     two versions of a crate, as one with transitive dependencies often
     does), with the note
     that the lock is committed on its own and that a crate which stays
     after the update needs its requirement in `Cargo.toml` raised first.
   - Not checked: the tool is absent, timed out, failed (its own `error:`
     line is the reason, or its first line when there is none), or
     answered in a shape the parser does not read.
     The line says which, and names what CI runs. An unreadable report is
     never rendered as clean: an empty stdout parses to an error, not to an
     empty list.

5. **It reports; it does not write.** No `--fix`, no `cargo update` run by
   `doctor`. The lock is committed on its own by a person (CLAUDE.md), and a
   command that rewrote a pinned file as a side effect of a probe would be
   the kind of surprise ADR 0009's clean-worktree rule exists to prevent.

Crate names and versions on the line come from the lockfile and the
registry, so they are printed through `plain` like everything else the
repository supplies.

## Consequences

- A person building a release reads the lock's standing from `doctor`
  before CI does, with the fix named.
- `doctor` can take up to 90 seconds longer in a repository with a
  `Cargo.lock` and a cold advisory cache. In a Python repository it is
  unchanged.
- cargo-audit's `--json` shape (the `rustsec` crate's `Report`, as 0.22.2
  writes it) and `deny.toml`'s `[advisories].ignore` are now read by
  trellis. A report shape change reads as "not checked", with the reason,
  never as clean; the fixture in `doctor_supply.rs` is the report for
  `main`'s lock at b49920e, run on 2026-10-25, and is the shape to update
  when CI's pinned cargo-audit moves.
- The policy is applied twice, by cargo-deny in CI and by this line, from
  the same ignore list. cargo-deny's other advisory settings (a severity
  threshold, say) are not read; `deny.toml` sets none today, and a line
  that reports more than CI fails on is the safe side.

## What is not built

- `cargo deny`'s policy (`deny.toml`: licences, bans, sources) is not read.
  Its `--format json` is a stream of diagnostics rather than a report, and
  licence policy is CI's question, not a machine's.
- `fuzz/Cargo.lock` and `desktop/Cargo.lock` are not read; the line is for
  the root's lock. Pointing `doctor --root` at either directory reads that
  one.
- No self-check of the installed binary against a newer release. That is
  a different question (ADR 0052 §7) and was not asked.

## Alternatives rejected

- **`trellis doctor --update-lock`**, running `cargo update -p` for each
  finding and leaving the change for review. Rejected by the owner: the fix
  is a commit a person makes, and `doctor` writing to the repository would
  need a guard that it never runs during `trellis run`.
- **cargo-deny instead of cargo-audit.** It applies `deny.toml` itself and
  so would match CI's first step exactly, but its JSON is one diagnostic
  per line with the crate inside a graph, and the parser would be the
  largest part of the change. cargo-audit's report is one object with the
  crate beside the advisory, and the ignore list is one TOML array to read.
- **cargo-audit's own verdict (its exit code).** Rejected: it passes the
  lock that failed CI.
- **Printing `lockfile: none` in a Python repository.** Rejected: see
  decision 1.
- **Running the check inside `trellis run`.** Rejected: a run's checks are
  the repository's own (ADR 0004), and a network fetch inside a run's
  budget would be a second clock.

## Reversibility

Delete `crates/trellis-cli/src/doctor_supply.rs`, its `mod` line in
`crates/trellis-cli/src/main.rs`, the one `extend` in `repo_lines`,
`crates/trellis-cli/tests/doctor_lockfile.rs`, the note in
`docs/COMMANDS.md` and this ADR's row in `docs/adr/README.md`. Nothing else
reads the line.

## What would change it

- cargo-audit changing its report shape: update the fixture and the pointer
  paths in `parse`.
- A second lock becoming the release's (desktop/, say): read both, each on
  its own line.
