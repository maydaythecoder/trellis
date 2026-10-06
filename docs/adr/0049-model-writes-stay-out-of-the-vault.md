# ADR 0049 — Model writes stay out of the configured vault

**Status:** Accepted (owner-directed). Built (the governance refusal #357,
the run and process-agent wiring #361 and the carry-back test #358; #363
records it in ADR 0022 §3 and the CLAUDE.md invariant list).
**Date:** 2026-10-06
**Decided by:** the session, under the owner's 2026-10-04 delegation of the
upstream plan's open questions, option A of APEX-08's question (#356).
**Extends:** the owner's #30 answer of 2026-09-28 (ADR 0022 §3) and the
CLAUDE.md invariant "Model writes never land in `.trellis/` or `.git/`".
It narrows what a model may write and loosens no rule.
**Reference design:** Apex (`boriemannetje/apex-notes` at f449644a), whose
hierarchy prompt (APX-36, web/appShell.ts:89, 100-110, 3205-3265) hands an
outside agent the whole SKILL.md and asks it to rewrite the vault's notes.
ADR 0021 row 35–37 excludes that prompt.

## Context

The owner's answer on #30, recorded in ADR 0022 §3, is that "vault notes stay
human-authored, and the model gets no note-write tool". The reason given: a
note is prompt input to later runs (ADR 0017), so a model-written note is the
model writing its own future context with no executed check behind it.

What the code does on 2026-10-06:

- The model has no note tool.
- Nothing refuses a note write made through `edit` or `write_file`, or carried
  back from `bash`, a custom tool, a hook or a process agent. A run
  (crates/trellis-cli/src/run_cmd.rs, around lines 821-835) and a process
  agent's judge (crates/trellis-cli/src/process_agent.rs, `Judge::load`) build
  their write refusals from the governance rule documents and
  `[governance] deny_write` only.
- The module comment in crates/trellis-loop/src/notes.rs says the vault is
  read afresh because "a run can edit it".

So #30's sentence holds only where a repository lists its vault in
`deny_write` by hand, and the two texts disagree.

## Decision

Model writes are refused structurally at the configured vault's notes root.
This is the session's choice under the owner's 2026-10-04 delegation, and it applies the owner's #30 answer of 2026-09-28 to every write path, not only to a dedicated tool.

1. **Scope.** When `[context] vault` is set, the run adds the vault's notes root, trellis_vault::notes_root of the configured directory (its notes/ subdirectory when that is a directory, after the notes/ rule, else the directory itself), to its write refusals, beside `deny_write`.
2. **Every writer.** Every writer trellis judges is refused at the vault's notes root: edit, write_file, bash, custom tools, hooks and process agents, the last four through what they carry back.
3. **No new mechanism.** The last four meet the refusal in ADR 0032's throwaway-clone carry-back, as one more entry among the refusals every writer meets; no post-hoc git check is added.
4. **The reason.** Every such refusal names [context] vault, so a person reading the refusal sees which setting put the path off limits and is not sent looking for a `deny_write` rule that does not exist.
5. **A vault at the root.** A vault whose notes root is the repository root stops the run before the snapshot, since the refusal would cover the whole tree.
6. **What it amends.** This ADR amends ADR 0022 §3 (#30's sentence now holds for every write path) and adds one more place model writes never land to the CLAUDE.md invariant list. Both texts are changed by the records ticket (#363) once a test holds the refusal, since CLAUDE.md lists only invariants a test holds; until then they stand as written, and this ADR's status says the enforcement is landing.

## Alternatives rejected

- **Opt-in.** ADR 0022 would say #30 covers a dedicated tool only, and a
  repository lists its vault in `deny_write` by hand. That leaves #30's
  sentence true only where someone remembered to.
- **Withdrawing the sentence.** That reverses the owner's #30 answer of
  2026-09-28, which the delegation does not reach.

## Consequences

- A run in a repository with `[context] vault` set cannot leave a changed note
  behind by any writer trellis judges. A note a person wants changed is
  changed by that person, with `trellis vault` or an editor, as ADR 0022 §3
  says.
- A writer that reaches the repository by absolute path is not stopped by
  this, as it is not stopped by any refusal; that remains the sandbox's job
  (ADR 0032, "Limits").
- The notes.rs module comment is corrected to cite this ADR by the run ticket
  (#361).

## What would reverse it

The owner withdrawing #30's "vault notes stay human-authored", or preferring
the opt-in `deny_write` form. This ADR is then superseded, ADR 0022 §3 and the
comment in notes.rs are corrected instead, and the governance, carry-back
test, run and records tickets (#357, #358, #361, #363) close unbuilt.
