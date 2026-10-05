# Amendment to ADR 0021 and ADR 0022: the Apex re-inventory of 2026-10-04

**Status:** Proposed.
**Date:** 2026-10-04
**Amends:** ADR 0021 (Apex table; "Cosmetic and [ui] exclusions, itemised"), ADR 0022 §1, §3, §4 and §5.
**Inventory:** apex-notes `f449644a459b67c3b5f2d05ea676a48c1f63fe0e`, the commit ADR 0021 and ADR 0022 already cite. Upstream has not moved. The re-inventory has rows APX-01 to APX-55; rows 1 to 37 keep their numbers, 38 to 42 are numbered for the first time, and 43 to 55 are new.

## Context

The re-inventory found three kinds of gap.

- **Behaviour Apex has that trellis does not reproduce, and no row records:**
  - Apex refuses a symlinked `notes/` folder and symlinked workspace metadata (`src-tauri/src/lib.rs:236-243, 1443-1496`).
  - Apex never fails a Markdown write because of `dates.json` (`web/appShell.ts:1949-2103`).
- **Capabilities ADR 0021 never itemised:** rows 44 to 55, previously folded into rows 31, 21, 27 to 29 and 15 to 19, or missed.
- **Inventory statements the code contradicts:**
  - search has four fields, not three;
  - the limits of `annotations.json`;
  - Apex's reparent can rewrite the parent as well as the child;
  - Apex replaces an unparseable `layout.json`.

## ADR 0022 amendments

### §1, Root (re-inventory 2026-10-04)

A `notes/` entry that is a symbolic link is refused, with the message `notes folder cannot be a symlink`, by both the lenient reader (`Vault::open`) and the run gate. Apex refuses it the same way (`read_workspace_blocking`).

Before this amendment, trellis did not follow the link but fell back to the vault directory itself, so the two tools read different files for that layout. This is the one way a vault the gate accepted can now be refused. The goldens are unaffected.

### §3, Every write (re-inventory 2026-10-04)

- **Symlinked sidecars are refused.** A `manifest.json` or `layout.json` that is a symbolic link is refused before any note is written, as Apex refuses it ("Workspace metadata cannot be a symlink"), and the run gate reports a symlinked `manifest.json` as a violation. `annotations.json` is still never opened, and `dates.json` was already refused when it is a link.
- **An unparseable layout.json is left untouched and reported.** Apex's `read_layout_file` reads it as `{}`, so its next patch replaces the file with only the patched keys and loses every other position.

### §4, Search (re-inventory 2026-10-04; pending the owner's answer)

ADR 0022 §4 declined `searchResults.ts` without citing an ADR 0021 exclusion clause. The recommended answer is that `searchResults.ts` is reproduced in `trellis_vault::search` and used by `trellis vault search` only.

That port covers:
- the empty-query listing by title;
- index results with limit max(4 x limit, note count), trigrams always merged at a minimum of 0.12, and the index score `score*12 + bm25*4 + trigram*18`;
- exact, prefix, substring, all-token and subsequence matches on title (120), filename (86), path (58) and searchText (24);
- the compact-title (+26) and filename-prefix (+14) boosts;
- the top 12.

`search_notes` keeps the index ranking of ADR 0024 §6. The subsequence match over searchText would give a weak positive score to almost any short query on a long note, and the model would lose "no note matched".

### §5, dates.json (re-inventory 2026-10-04)

A vault edit never fails because of `dates.json`. In `dates::after_edit`:
- A touched note that cannot be read, seeded or reconciled keeps its previous entry, or gets none, and is named in the message.
- Every other touched note is still updated.
- A document that would exceed `dates.json`'s limits, or a save that fails, leaves the file's bytes unchanged. The message says `dates.json left unchanged` and that the note edit is written.

This is Apex's "date tracking never blocks Markdown" (README.md:69-71). The signature of `after_edit` is unchanged, so the `trellis vault` commands exit 0 after a landed edit instead of 1.

### §3, amendment #30 (re-inventory 2026-10-04; pending the owner's answer)

The sentence "vault notes stay human-authored" is enforced nowhere today. The run's write refusals come only from governance rules and `[governance] deny_write`, and `crates/trellis-loop/src/notes.rs` says "a run can edit it". The recommended answer adds the configured vault's notes root to the run's write refusals. Because that extends a CLAUDE.md invariant, it is written up in its own ADR (0049, the number #284 holds for APEX-08 in scripts/checks/adr-numbers.sh) and not here.

## ADR 0021 amendments

New Apex rows, each ending `(re-inventory 2026-10-04)`:

| Items | Disposition |
|---|---|
| 43 path safety for workspace files | W-A, landing under the ADR 0022 §1 and §3 amendments above |
| 44–47 workspace tabs, recent projects, project folder rename, project location chooser | excluded (#70); row 31's `vault init` is the built part |
| 48–49 paste plain text as a note, editor link rendering | excluded [ui], as rows 21, 27–29 |
| 50 per-line date labels and the note date header | excluded [ui]; `trellis vault dates` prints each note's created day and matching line count |
| 51 date tracking that never blocks Markdown | W-A, landing under the ADR 0022 §5 amendment above |
| 52 search result ranking layer | not built (ADR 0022 §4); an owner question on porting it is open |
| 53–55 rope connect gesture, hub and PageRank scoring, workspace chrome | excluded [ui], as rows 15–19 |

Amendments to existing rows:
- **Row 31:** tabs, recent projects, folder rename and the location chooser are rows 44 to 47.
- **Row 35–37:** `AGENTS.md` reaches the model as a context file under ADR 0025 (`crates/trellis-cli/src/context_files.rs`). trellis ships no Apex rules of its own. The writing skill's row waits on the owner's answer; the recommended answer is built in effect, through `trellis install` (ADR 0039) or a copy into `.trellis/skills/`, with no skill in the binary (ADR 0022 §6).
- **Itemised exclusions:** one line for each excluded row 44 to 50 and 53 to 55, plus "A desktop surface adopted by a later ADR re-dispositions these rows."

## Consequences

- The run gate refuses a vault whose `notes/` is a symbolic link, where it used to read the parent directory.
- `trellis vault` edits exit 0 when `dates.json` cannot follow them, and say so in one line.
- No new dependency. `trellis-vault` depends on neither `trellis-model` nor `trellis-core`.
- No existing ADR 0021 disposition changes. Rows 43 and 51 move from "landing" to built when their tickets merge.
