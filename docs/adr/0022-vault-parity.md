# ADR 0022 — Apex vault parity: lenient reading, strict run gate, `trellis vault`

**Status:** Accepted. Amended in implementation: §3 (`reparent --no-parent`;
`rm` writes only what changes; the search limit), §5 (`dates --seed` may
create the file; a BOM changes anchors), §7 (added: `[context] dates` and
auto-created linked notes are not built), and "Evidence" (how the vectors were produced). Each amendment is
marked in place. Integrated afterwards: `[context] dates` (§7) and `vault
init` outside a repository (Consequences).
**Date:** 2026-09-27
**Reference design:** Apex Notes (`boriemannetje/apex-notes`, MIT), commit
`f449644a459b67c3b5f2d05ea676a48c1f63fe0e`. The behaviour below was read from
these files at that commit:
- `web/notes/noteParser.ts`
- `web/noteRefs.ts`
- `web/graphModel.ts`
- `web/notes/noteDates.ts`
- `web/searchIndex.ts`
- `web/notes/noteComposer.ts`
- `web/notes/noteValidation.ts`
- `web/appShell.ts` (the create, link, reparent, retitle and delete flows)
- `src-tauri/src/lib.rs`
- `skills/apex-notes-writing/SKILL.md`

No Apex code runs in trellis. The algorithms are re-implemented in Rust, and
where the TypeScript is the only statement of the behaviour, the tests hold
the Rust to vectors produced by running that TypeScript (see "Evidence").

## Context

ADR 0017 took the Apex file format and nothing else: two frontmatter keys, one
parent per note, and a sorted manifest. It resolves links by file stem only,
and any departure from the format refuses the vault. ADR 0021 assigns the rest
of Apex's non-UI behaviour to this wave: how it reads notes, how links
resolve, its graph index and issues, its search, its editing operations, and
`dates.json`.

Two requirements pull against each other. Apex reads vaults leniently: a
missing title falls back to a heading, `~` means null, and `[[Title]]`
resolves. A vault written by Apex, or by a person, should be readable by
trellis. The run gate of ADR 0017, on the other hand, exists so that what the
model is shown is a pure, deterministic function of a well-formed vault. Its
three golden files are the specification of the selection rule.

## Decision

### 1. One lenient reader, and a strict gate on top of it

**Reading follows Apex** (`noteParser.ts`, `lib.rs#walk_markdown_files`):

- **Root.** If the vault directory has a `notes/` subdirectory, that is the
  root; otherwise the directory itself is.
- **Files.** `.md` files are read recursively. The extension is matched
  case-insensitively. Names starting with `.` are skipped, and so are
  symlinks, both files and directories.
- **Frontmatter.** Present when the file starts with `---` and a later
  `\n---` exists. Keys match `^[A-Za-z0-9_-]+:`, and a value has matching
  outer `"…"` or `'…'` stripped.
- **Title.** The frontmatter `title`, else the first `# H1` in the body, else
  the basename.
- **Null parent.** `parent` is null when it is `""`, `null`, `undefined` or
  `~`, compared case-insensitively after trimming.
- **Body links.** `[[stem]]`, `[[path/stem]]`, `[[Title]]` and
  `[[target|label]]`.
- **Resolution** follows `graphModel.ts#resolveNotePath` exactly, in this
  order:
  1. an exact path;
  2. the path plus `.md`;
  3. the alias map under `normalizeKey(ref)`;
  4. the alias map under `normalizeKey(slugify(ref))`.

  Each note's alias keys are its path, its path without `.md`, its basename,
  its title, and the slug of each of the last three, all normalised. A key
  that two notes share goes into neither note's alias map. The two path
  shortcuts still win: `[[task-b]]` resolves to `task-b.md` even when
  `sub/task-b.md` also exists.

Line endings are normalised to LF and a BOM is stripped before any of this
(ADR 0017's rule). Apex does neither: its frontmatter regex fails on a
trailing `\r`, so a CRLF note has no title in Apex. On this point trellis is
deliberately more lenient than its reference.

Titles in quotes are decoded as JSON strings, as ADR 0017 does, with one
addition: single quotes are stripped as well. Apex's `stripQuotes` removes the
quotes but does not unescape, so a title Apex writes as `"A \"B\""`, which is
what its own `escapeYaml` produces, reads back in Apex as `A \"B\"`. Trellis
reads it as `A "B"`. So trellis round-trips what `composeRaw` writes, and Apex
does not.

**The run gate stays strict about the schema.** `trellis run` (through
`validate` and `compile`) still refuses a vault in which any note:

- has a missing frontmatter delimiter;
- has an unparseable line;
- has a key other than `title` and `parent`;
- has a duplicate or missing key;
- has a parent that is neither a null form nor `"[[…]]"`.

The gate also still refuses a vault whose parent does not resolve, whose
parents form a cycle, or whose manifest is wrong. Every message is
byte-identical to ADR 0017's, and so is every existing test, unmodified.

**What changes at the gate is additive:**

- Parents and body links resolve through the resolver above rather than by
  stem alone. `parent: "[[Project]]"` is now accepted where it names a title.
- A body link that resolves by title is included as a reference rather than
  listed as unresolved.
- A link whose alias key is shared lists every candidate.
- The four null forms are accepted as null.
- The `notes/` root, dotfile skipping and the case-insensitive extension
  apply to the gate too, so the gate and the reader see the same files.

The **focus** of `trellis run --focus <stem>` still resolves by file stem
alone, as ADR 0017 specifies. So a focus that names a basename two notes share
is ambiguous and says so, while a body link with the same text resolves by the
path shortcut. The two differ on purpose. The focus is an operator's argument,
and guessing between two notes is worse than refusing. The link is note
content, and it resolves the way Apex resolves it.

The fixture's links are all plain stems that resolve by path, so the three
goldens are byte-identical.

**`trellis vault check --fix` is how a lenient vault becomes one the gate
accepts.** It rewrites a note, through `composeRaw`, only when the gate rejects
its frontmatter. The cases are:

- an extra key, which Apex's `composeRaw` also drops;
- a duplicate or unparseable key line;
- a missing key or missing frontmatter, where the title falls back as above;
- a parent in a form other than `"[[…]]"` or `null`.

It writes a resolvable parent as `"[[basename]]"`, as Apex does, or as the
path without `.md` when the basename is shared, because Apex would then write
a link that resolves to nothing.

It rewrites `manifest.json` only when its set, order or shape is wrong, in
Apex's format: pretty-printed, sorted, with a trailing newline. It removes
`layout.json` keys that name no note.

A note or file that is already correct is never touched. Apex's
`removeDeletedNoteLinks` rewrites every note that differs from `composeRaw`
output, including only in the blank line after the closing `---`. Trellis
does not, because a check that dirties every canonical note on a clean vault
would dirty the worktree a run requires clean (ADR 0009).

### 2. Graph index and issues

`trellis_vault::Graph` is `createGraphIndex`:

- **Tree edges** come from resolved parents.
- **Weighted reference edges** come from every body `[[…]]`, counted with
  multiplicity. A self-link is excluded, and so is a link that duplicates a
  parent–child edge in either direction.
- **`refs_in` and `refs_out`** carry weights.
- **Roots** are the notes whose `parent` is null. A note with an unresolved
  parent is not a root, because Apex's `getRootPaths` filters on the
  reference, not on its resolution.
- **Depth** is a BFS from the roots. A note under an unresolved parent, or on
  a cycle, has no depth. `graph` prints it as absent, never as 0.

The issues:

- **`missing-parent`**: the parent reference resolves to nothing. It uses
  Apex's message; when the reference is ambiguous, the candidates are
  appended.
- **`cycle`**: Apex's `findCycles`, reported once per member set.
- **`duplicate-alias`**: an alias key shared by more than one note, reported
  once per set of notes with the keys they share. **Apex defines this type and
  never raises it.** Trellis raises it because a shared key is exactly the
  case where a link silently resolves to nothing.
- **`disconnected`**: **a trellis definition.** Apex raises `disconnected` for
  a tree edge whose endpoint is outside the vertex set, which cannot happen
  when edges and vertices come from the same list. Trellis raises it for a
  note with no resolved parent and no children, which is the complement of
  the report line's `connected` count. It is informational, because a loose
  note is valid (SKILL.md: "Multiple roots/loose notes in one folder are
  valid"), and it never changes an exit code.

Body links are scanned two different ways, as in Apex, and the two are not
unified:

- **The graph** uses `\[\[([^\]]+)\]\]` with no masking, the way
  `graphModel.ts#parseWikiRefsWithMultiplicity` does.
- **Rewrites** (`retitle`, `rm`) use `appShell.ts#rewriteBodyWikiLinks`:
  fenced code is masked, `![[…]]` is skipped, the inner text is limited to
  1–240 characters, and there is no newline.

### 3. `trellis vault`

This is one new subcommand, in `crates/trellis-cli/src/vault_cmd.rs`. It
takes the vault from `--vault <dir>`, or else from `[context] vault`. A
`--vault` path is taken as given; `[context] vault` is taken relative to the
repository root. A note argument is any form the resolver accepts: stem,
path, title or slug.

| Command | Behaviour |
|---|---|
| `check [--fix]` | Prints the SKILL.md report line `notes=N parentless=N connected=N manifest_missing=N manifest_extra=N fixes=[...]` from the lenient reader, then the issues, the size warnings and the run gate's verdict with every violation. Exits 1 when the run gate would refuse the vault, and 0 otherwise. |
| `search <query> [--limit N]` | `SearchIndex.search`: BM25 with k1 1.2 and b 0.75, and a trigram Jaccard fallback weighted 0.35 with a minimum of 0.18. |
| `graph <note>` | Lineage from the root, children, `refs_out` and `refs_in` with weights, and depth. |
| `new <title> [--parent <note>]` | `getAvailableDisplayTitle` (" 2", " 3", … on a title already taken), then `getAvailableNewNotePath` (slug, then `-2`, `-3`, …, compared case-insensitively), then `createNoteRaw`. |
| `reparent <note> (<parent> \| --no-parent)` | Refuses a parent that is the note itself or one of its descendants. Rewrites only the child. Apex's layout-driven "connection plan" can also rewrite the parent, and that is UI behaviour, not taken. |
| `link <from> <to>` | `appendBodyReference`: trailing whitespace is trimmed, then a blank line, `[[key]]` and a newline are appended. It is a no-op when a body link already resolves to the target. `key` is the basename, or the path without `.md` when the basename is shared. |
| `retitle <note> <title>` | Sets the title, then rewrites every body wikilink that resolved to the note, under the index built before the change, into `[[New Title]]`, keeping `\|label`. |
| `rm <note>` | Deletes the file. Body links that resolved to it become their label, or their reference when there is no label. Children get `parent: null`, which is Apex's code; its docs say children keep their broken parents. Git is the undo; there is no OS trash. |
| `init <dir>` | Creates `<dir>` with `manifest.json` (`[]`) and `layout.json` (`{}`). It refuses when either already exists. Apex creates `<dir>/notes/`; `init <dir>/notes` gives that layout. |
| `dates [--seed]` | See §5. |

**Every write:**

- keeps `manifest.json` sorted (UTF-16 order, as the gate checks) and equal
  to the files on disk;
- removes `layout.json` keys for deleted notes and leaves every other key;
- never touches `annotations.json`;
- writes notes with exactly two frontmatter keys, in `composeRaw`'s form.

A note file is written only when its bytes change.

*Amended in implementation:*

- **`--no-parent`, not `--root`.** The binary's global `--root <PATH>` owns
  that name. Clap panicked at parse time on the collision, so
  `the_vault_subcommand_parses_under_the_real_cli` now parses through the
  real `Cli`.
- **`rm` rewrites only the notes whose links or parent change.** Apex's
  `removeDeletedNoteLinks` writes every note whose `composeRaw` output
  differs from its file. That would reformat unrelated notes, for the reason
  given for `--fix` in §1.
- **`search` defaults to 12 results**, the limit Apex's search box uses.
- **The note argument resolves as a link would**, so `graph "Task A"` and
  `graph task-a` name the same note. The run's `--focus` keeps stem-only
  resolution (§1).

The only automatic repair is `--fix`; the other commands refuse on a problem
and name it. `trellis vault` is a command the human runs. It is not a tool the
model has, so it does not pass through the tool gate (ADR 0018), and it
performs no clean-worktree check: the edit is the human's, and git shows it.

### 4. Search

The search index is `searchIndex.ts`'s `SearchIndex`. Apex builds it from
parsed notes, so each note has **four** fields:

| Field | Weight | Content |
|---|---|---|
| title | 3 | the title |
| path | 1.4 | the path |
| searchText | 1 | the lowercased `title path raw` |
| body | 1 | the body |

The inventory lists three fields. The code has four, and trellis follows the
code.

Search text is normalised as NFKD, with U+0300–U+036F stripped and the result
lowercased. Tokens are runs of `[a-z0-9]` of length 2 or more; trigrams are
taken over the normalised text.

Two Apex settings are reproduced:

- **Body indexing is dropped** above 1,000 notes, above 25 MiB, or when any
  note is 2 MiB or more.
- **Trigram indexing is dropped** above 3,000,000 UTF-16 characters.

The command prints which mode is in effect when either is reduced.

Ties are broken by title, then path, compared by `localeCompare` with base
sensitivity. Trellis approximates that comparison as a comparison of the
normalised, lowercased strings. The approximation can differ from a real
locale collation on punctuation order, and on nothing else in these fields.

The UI layer in `searchResults.ts` is not reproduced: it adds substring and
subsequence boosts on top of the index.

### 5. `dates.json`

**Reading and validation are exact.** The shape is
`{"version":1,"notes":{path:{"created":{"day","estimated"},"lines":[{"anchor","day","estimated"}]}}}`.
Within it:

- unknown and missing fields are rejected at every level;
- `day` is required and may be null; a non-null day is a real `YYYY-MM-DD`
  calendar day;
- `anchor` is 32 lowercase hex characters;
- a path is relative and `/`-separated, has no `.`, `..`, empty segment,
  backslash or NUL, and ends in `.md` in any case.

The limits are:

| Limit | Value |
|---|---|
| Notes | 20,000 |
| Lines per note | 100,000 |
| Lines in total | 200,000 |
| Bytes, compact | 16 MiB |

**The anchor is reproduced bit for bit.** `lineAnchor` hashes UTF-16 code
units with four 32-bit FNV-style lanes: seeds `811c9dc5`, `9e3779b9`,
`85ebca6b` and `c2b2ae35`, primes `0x01000193 + 2h`, and an xor-shift by 13.
This is JavaScript `Math.imul` and `>>> 0` arithmetic, which is exactly
wrapping `u32` arithmetic. The Rust side of Apex only validates the anchor's
format and never computes one, so the TypeScript is the whole specification.

Because the anchors match, trellis **seeds and reconciles** dates rather than
only reading them:

- **`trellis vault dates`** validates the file and prints, per note, the
  created day and how many lines still carry an anchor that matches the
  current text.
- **`trellis vault dates --seed`** does what Apex does on opening a folder:
  - a note with no entry is seeded from its file times (`seedNoteDates`,
    including the `*Written:*` and `*Added:*` markers);
  - a note whose lines changed is reconciled (`reconcileNoteDates`) with the
    stamp {mtime day, estimated};
  - an entry for a file that no longer exists is dropped.

The **body** hashed is Apex's lenient body: the text after `\n---` and its
leading blank lines, split on `\n` with a trailing `\r` removed. It is not the
gate's body.

A day is a **local** calendar day, as JavaScript's `Date` computes it. The
vault crate's API takes days as strings, and the CLI computes them with `jiff`
from the system time zone.

**`trellis vault` edits maintain an existing `dates.json`** the way Apex's
`invokeNative` does:

- a new note is seeded as new, with today's date, not estimated;
- a rewritten note is reconciled with {today, not estimated};
- a deleted note's entry is removed.

When `dates.json` does not exist, nothing date-related happens: no file is
created, and no output mentions dates. When it exists but fails validation,
it is left untouched and the command says so. **A file that fails validation
is never overwritten**, `--seed` included.

*Amended in implementation:*

- **`trellis vault dates --seed` creates `dates.json` when it is absent.**
  This is the one exception to the rule above. The command asks for the file
  by name, and Apex creates the file whenever it opens a folder.
- **A note that starts with a BOM hashes differently.** Trellis strips the
  BOM before parsing (ADR 0017). Apex does not strip it, so in Apex that note
  has no frontmatter and its whole text is the body. So the two tools anchor
  different lines for such a note. No other input is known to diverge.

**Writes are atomic, mirroring `atomic_write_file`:**

1. Serialise compactly, with keys in byte order, as `serde_json` does with
   Apex's `BTreeMap`.
2. If the bytes equal the existing file, do nothing.
3. Write a temporary `.dates.json.<pid>.<n>.tmp` beside the file, created
   exclusively, and sync it.
4. Re-read the target and require it to be byte-identical to the snapshot
   that was validated; if it is not, delete the temporary file and fail.
5. Rename over the target.

The directory sync after the rename is done on Unix only, because Windows
cannot open a directory as a file.

### 6. The writing skill

ADR 0021 row 35–37 says W-A ships Apex's writing contract as a built-in
skill. **This wave does not.** Skills are data in the repositories that use
them (ADR 0019), and a built-in one would be the first skill trellis carries
in its binary. Instead, `trellis vault check` prints the exact report line
that SKILL.md asks an agent to end with, so an agent following Apex's skill
can use trellis to verify its work. The owner can override this.

### 7. `[context] dates` is not built (integrated since; see below)

*Added in implementation.* The brief made it optional to annotate compiled
note headers with each note's created day, behind `[context] dates = true`.
The annotation is not built. Reading the flag means changing `cmd_run` in
`main.rs`, and this wave's edits to that file are limited to registering the
subcommand. Nothing is stubbed: no config key is read, and the goldens are
unaffected.

*Integrated.* `[context] dates = true` is now read. With it, `cmd_run`
compiles through `trellis_vault::compile_with_dates`, which annotates each
lineage and reference heading whose `dates.json` entry has a created day:
`### Task A (task-a.md, created 2026-03-01) [target]`, with `, estimated`
when Apex marked the day so. Children lines are not annotated.
- **The goldens are untouched by construction.** `compile` is
  `compile_with_dates(…, None)`. Tests hold all three goldens byte-identical
  with a `dates.json` present and the flag off, and with the flag on and a
  document that dates none of the notes: a note with no entry, or a null
  day, gets the plain heading.
- **`dates.json` absent with the flag on** compiles the plain note, and the
  `repo:` line says `dates on, but no dates.json; headings not dated`.
- **Present and invalid with the flag on stops the run before the
  snapshot**, like an invalid vault: the configuration said the headings
  would be dated, and sending them undated without a word is the failure
  ADR 0025 §1 names.
- The annotation counts toward the cap, since it is sent.
- `[context] dates` layers from the user config like the other `[context]`
  fields (ADR 0025 §7).

**Auto-creating linked notes (inventory item 22) is not built either.** ADR
0021 row 20, 22–26 assigns it to this wave. Apex creates a loose note when an
editor autosave finds a *newly typed* `[[X]]` that resolves to nothing. A CLI
has no "newly typed" event. Doing it on `check --fix` instead would create a
note for every unresolved link in the vault, notes the writer never asked
for. `trellis vault new` is the explicit form.

## Alternatives rejected

**Two parsers, strict for the gate and Apex's for everything else.** With two
parsers, `trellis vault graph` and the compiled context could disagree about
what `[[Project]]` points to. One resolver serves both, and the strictness
lives only in the schema checks layered on top.

**Resolving the focus through aliases.** Rejected in §1: an operator's
argument that names two notes should fail, not pick one. The test
`an_ambiguous_stem_names_every_candidate` holds that line.

**Rewriting every non-canonical note on `--fix`.** Rejected in §1: a clean
vault would come out dirty.

**Read-only `dates.json`.** This was the fallback if the anchors could not be
matched. They were matched (§5, and "Evidence").

**`chrono` for the local day.** `jiff` is already in the lockfile, through
`tui-logger`.

**An OS trash for `rm`.** The `trash` crate is excluded by ADR 0021, and the
vault is in git.

## Consequences

- **New dependencies.**
  - `unicode-normalization` in `trellis-vault`. Apex's `slugify` and search
    normalise to NFKD before stripping combining marks. Without it, `Café`
    slugs to `caf` and fails to resolve against Apex's `cafe`.
  - `jiff` in `trellis-cli`, for the local calendar day. The standard library
    has no time zones.
  - `serde` (derive) in `trellis-vault`, to deserialise `dates.json` with
    unknown fields rejected. It is already a workspace dependency.
- **The CLI grows by one subcommand, `vault`, with ten actions.** This is
  owner-directed, like ADR 0020.
- **`trellis vault` requires a repository**, like every command except `init`
  and `model`. So `trellis vault init` outside a repository needs `--root`.
  *Integrated:* `vault init` is now exempt in `needs_repo`, since it creates
  a vault a person may make before there is a repository to hold it. Every
  other `vault` action still refuses outside one
  (`vault_init_works_outside_a_repository_and_other_vault_actions_still_refuse`).
- **The gate accepts vaults that ADR 0017 refused**: title-form parents, the
  null forms, and a `notes/` root. The set of files it reads changes in two
  ways:
  - dotfiles and dot-directories are now skipped rather than validated;
  - a file ending `.MD` or `.Md` is now a note, which the manifest must list.

  The second is the one way a vault the gate accepted before can now be
  refused.
- **Size warnings use Apex's `>=`**: 1,000 notes, 50 MiB in total, and 2 MiB
  for one note. The brief said `>`; the code says `>=`, and the code is the
  reference.
- **Compile time on a large vault is not measured.** ADR 0017's pending
  measurement still stands, and the reader now builds an alias index as well.

## Evidence

**Anchor vectors, hand-computed.** For `""`, no character is hashed, so the
anchor is the four seeds: `811c9dc59e3779b985ebca6bc2b2ae35`.

For `"a"` (0x61, index 0), lane 0 runs:

| Step | Value |
|---|---|
| `0x811c9dc5 ^ (0x61 + 1·1)` | `0x811c9da7` |
| `× 0x01000193 mod 2³²` | `0xa7000000 + 0x400c2de5 = 0xe70c2de5` |
| `^ (x >>> 13)` | `0xe70c2de5 ^ 0x00073861 = 0xe70b1584` |

Every step is written out in the test comment.

**Anchor vectors, generated.** *Amended in implementation: the method
changed.*

The method:

1. Each Apex source was fetched unmodified at `f449644a` with
   `gh api repos/boriemannetje/apex-notes/contents/<path>?ref=<sha>` and laid
   out in its original `web/` tree.
2. Short `.mjs` scripts imported the `.ts` modules directly. Node v25.6.0
   strips TypeScript types natively.
3. Each script printed the vectors below.

| File | Vectors |
|---|---|
| `crates/trellis-vault/src/dates.rs` | `lineAnchor` for nine strings; `seedNoteDates` with markers; two `reconcileNoteDates` cases; four documents `normalizeDateDocument` rejects |
| `src/refs.rs` | `slugify`, `normalizeKey`, `parseWikiTarget`, `getNoteAliasKeys` |
| `src/search.rs` | `createSearchIndex(…).search(q, {limit: 12})` scores for four queries |
| `tests/parity.rs` (`graph_matches_apex_vectors`) | `createGraphIndex` parents, `resolvePath`, `refsOut`, `roots`, `derivedLevels`, `duplicateAliases` |

The anchor strings cover ASCII, a BMP character (`é`) and a surrogate pair
(`😀`).

The Rust tests passed against these vectors on their first run. Before that
run, one expectation had been written by hand before the vectors existed, and
it was wrong: `parseWikiTarget("[[notes/x/y.md | Label ]]")` yields `x/y.md`,
because the trim runs after the `.md` strip. The Rust code already did this.
The expectation was corrected to Apex's output.
