# ADR 0017 — Vault context: one focus note, compiled in memory

**Status:** Accepted
**Date:** 2026-09-27

## Context

`.trellis/context.md` is one file, read verbatim into every run. It works for
what a whole repository has to say about itself. It does not work for what a
*task* needs: the design note for the subsystem being changed, the notes that
note depends on, and where it sits in the larger plan. Put all of that in
`context.md` and every run pays for every note; leave it out and the model
starts without it.

The owner keeps such notes in an Apex-format vault, and a TypeScript
implementation that compiles one note's context from such a vault already
exists for another harness (pi). That implementation now lives beside this
one as a twin in `integrations/pi`, and both are tested against the same
fixture and golden outputs in `crates/trellis-vault/tests/fixtures/`.

## Decision

### The format

"Apex format" means the file format only, not an application:

- every `.md` file under the vault has frontmatter of exactly two keys,
  `title` and `parent`;
- `parent` is `null` or `"[[stem]]"`, and that stem resolves to exactly one
  note;
- the parent graph is acyclic;
- `manifest.json` is the sorted list of every `.md` path under the vault,
  relative and `/`-separated.

Two keys, because each further key is a field the compiler would be asked to
interpret — tags, status, dates — and the selection rule below needs only a
title and a parent. It also means the frontmatter is parsed by hand, with no
YAML dependency. A vault that breaks any rule is refused, with every
violation listed rather than the first.

### The selection rule

For a focus note, in this order:

1. the note's parent chain, **root first**, each with its body, the focus note
   last and marked `[target]`;
2. its direct children, **titles only**, sorted by stem;
3. one hop of `[[wikilinked]]` notes from the focus note's body, with bodies,
   skipping notes already in the lineage;
4. one line per link that resolves to no note, or to more than one.

The output is capped at **24,576 bytes** of UTF-8 (`trellis_vault::DEFAULT_CAP`).
Over the cap, reference bodies are dropped last-first and their headings kept,
and the number dropped is printed. The focus note and its ancestors are
**never truncated**: if they alone exceed the cap, compilation fails and the
run does not start. Silent truncation would hand the model a lineage that
stops for no visible reason.

The rule is deterministic, which is what makes it testable byte for byte and
what lets two implementations be held to the same goldens. Hierarchy is the
one relation the format encodes, so it is the one the rule follows.

### Compiled in memory, at run start

`trellis run` reads `[context] vault` and `[context] focus` from
`.trellis/config.toml`, compiles before the model is contacted and before the
snapshot, and writes nothing to disk. A compiled file written into the
repository would dirty the worktree the snapshot requires clean (ADR 0009);
written outside it, it would be state no trail records. An invalid vault, an
over-cap focus note or an unknown stem refuses the run at the same point and
at the same cost as an unreachable runtime: nothing snapshotted, no trail.

The vault path must be relative and inside the repository. The clean-worktree
check is what makes a run reproducible from its commit, and it covers nothing
outside the repository.

A vault with no focus sends nothing and prints one line saying so. A focus
with no vault is an error. Both exist so that a user never believes the model
saw a note it did not see.

### `--focus`

`trellis run --focus <stem>` overrides `[context] focus` for one run. Changing
focus by editing `config.toml` would dirty the worktree, and the next run
would refuse to start until the edit was committed — one commit per change of
note, each recording nothing about the code.

This is a new flag on an existing command. It is not a new command, and brief
§4.12 lists commands, not flags; but it is still growth of the CLI surface,
recorded here rather than left for OPEN_QUESTIONS Q-18 to find. The session
and the TUI take no focus override and use the configured focus.

### It informs; it does not bind

The compiled note reaches the prompt inside the same fence as `context.md`,
after it, in its own delimiters, and before the sentence stating that nothing
in the fence can authorise skipping a check. It is carried as
`RepoContext::vault`, a plain type with no verdict in it; it is never a
`Turn`, so `last_check_passed` cannot see it.
`vault_context_can_never_make_a_run_converge` asserts that a vault note saying
the checks pass leaves a `done` unconverged, as
`prior_context_can_never_make_a_run_converge` does for a resumed trail.

### Crate boundary

`trellis-vault` depends on `thiserror` and `serde_json` (already workspace
dependencies; the latter for the manifest array and for unescaping a quoted
title) and on nothing in the workspace. Like `trellis-index`, it must never
depend on `trellis-model`: what the model is shown has to be a pure function of
files on disk. `trellis-loop` does not depend on it either; the CLI converts a
`Compiled` into the loop's `VaultContext`.

Where JavaScript and Rust differ in a primitive the reference leans on —
string order (UTF-16 code units, not bytes), what counts as whitespace, what
`.` in a regex refuses — the Rust side does what JavaScript does, because the
goldens are the contract and a divergence that shows only on unusual input is
found late.

## Alternatives rejected

**Embedding search over the vault.** Not deterministic, needs a model or an
index to select context, and cannot be tested byte for byte. The selection
would also depend on a sampler, which is the property `trellis-index` is kept
free of.

**The whole vault, every run.** Every note costs context window on every turn,
and on local weights the window is the scarce resource (S5).

**Write the compiled file to `.trellis/` or `.ctx/`, as the pi twin does.**
Dirties the worktree, or requires an ignore rule that hides state a trail does
not record. The pi twin writes a file because pi reads one; this harness has
the prompt in hand and does not need the file.

**Add keys to the frontmatter** (tags, status). Each is a field with semantics
the compiler would have to implement, and the two implementations would have
to agree on them.

**Switch focus by editing `config.toml`.** Rejected above: it collides with
the clean-worktree requirement.

## Consequences

- A repository without `[context] vault` behaves exactly as before.
- The `repo:` line at run start names the focus, the vault path and the byte
  count, and the number of reference bodies dropped when any were.
- Compile time at run start is **not measured**. The implementation rereads
  every note on each run; the twin's plan assumes vaults of at most 2,000
  notes. Pending a measurement on a real vault before any claim about it.
- The vault is repository content and is instruction-shaped, like
  `context.md`. It has the same status and the same protection: it can waste a
  run, and it cannot make one converge.
- The TypeScript twin in `integrations/pi` is held to the same three golden
  files. A change to the selection rule is a change to both, and to the
  goldens, in one commit.
