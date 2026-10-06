# apex-notes capability inventory (commit f449644a, MIT)

The inventory was made by reading the repository through `gh api`; nothing was run. Tauri 2 desktop app. It has no CLI, no AI API calls, no export, no tags, no `aliases:` key, and no way to rename or move a file. The Rust commands live in `src-tauri/src/lib.rs` and the UI in `web/appShell.ts`. Items marked [ui] exist only in the desktop UI. Places where the code and the docs disagree are marked (gap).

## A. Note format
1. **Frontmatter:** `title` and `parent` only. Each line is parsed with a regex, not a YAML parser, and surrounding quotes are stripped. Source: `AGENTS.md`, `web/notes/noteParser.ts`.
2. **Title fallback:** frontmatter `title`, then the first `# H1`, then the file basename. Source: `noteParser.ts`.
3. **Parent edge:** `parent: "[[stem]]"` or null. The values `""`, `null`, `undefined` and `~` all mean null. The app always writes `[[basename]]`. Source: `noteParser.ts`, `noteComposer.ts`.
4. **Rewrites keep only two keys.** Every write the app makes goes through `composeRaw`, which emits only `title` and `parent`. A legacy `level` key, or any other key, is dropped. Source: `noteComposer.ts`.
5. **Body wikilinks:** `[[stem]]`, `[[path/stem]]`, `[[Title]]`, `[[target|label]]`. They are reference edges only. A repeated link adds weight to the edge. Source: `web/noteRefs.ts`, `web/graphModel.ts`.
6. **Implicit aliases.** A note resolves by any of these keys: its path, its path without `.md`, its basename, its title, and a slug of each. There is no `aliases:` key. A key shared by two notes resolves to neither. Source: `noteRefs.ts#getNoteAliasKeys`, `graphModel.ts`.
7. **Root folder.** If the opened folder has a `notes/` subfolder, that subfolder is the root; otherwise the folder itself is. `.md` files are read recursively, skipping dotfiles and symlinks. Source: `lib.rs#walk_markdown_files`.

## B. Sidecar files (all in the notes root)
8. **`manifest.json`.** A sorted, pretty-printed array of relative `.md` paths, e.g. `["a.md","sub/b.md"]`. It is rewritten after every create or delete, and created on open if missing. (gap) The app never reads it.
9. **`layout.json`.** Positions only, never hierarchy: `{"<path>.md": {"x"?:num,"y"?:num,"dx"?:num,"dy"?:num}}`. `x`/`y` are absolute; `dx`/`dy` offset the automatic position. The `write_layout_patch` command takes `{path: pos|null}`, and null deletes that entry.
10. **`dates.json`.** Unknown fields are rejected on both the Rust and the TypeScript side.
    ```json
    {"version":1,"notes":{"<rel/path>.md":{
      "created":{"day":"YYYY-MM-DD"|null,"estimated":bool},
      "lines":[{"anchor":"<32 lowercase hex>","day":"YYYY-MM-DD"|null,"estimated":bool}]}}}
    ```
    - There is one entry per body line (the text after the frontmatter), blank lines included.
    - `anchor` is a 128-bit hash of the line text (4×32-bit, FNV-style), so it identifies content, not position. The exact algorithm is in `web/notes/noteDates.ts` and `lib.rs`.
    - `day` is a local calendar day. `estimated:true` means the day was inferred from file times.
    - Limits: 20k notes, 100k lines per note, 200k lines in total, 16 MiB.
    - Writes are atomic: write a temp file, check the existing bytes are unchanged, then rename. A file that fails validation is never overwritten.
    - Source: `lib.rs#validate_dates`, `web/notes/noteDates.ts`.
11. **Seeding dates.**
    - A new note gets today's date, not estimated.
    - An existing file gets `created` from its birth time or mtime, and every line gets the mtime day, estimated.
    - `*Written: YYYY-MM-DD*` and `*Added: …*` marker lines override the estimates from that line to the next marker; a heading directly above the marker is included. `Written` also sets `created`.

    **Reconciling dates** after a change (`reconcileNoteDates`):
    - Lines in the common prefix and common suffix keep their dates, matched by anchor.
    - An interior line keeps its date only if its anchor is unique on both sides and still in order.
    - Every other line gets a new stamp: today for an edit made in the app, or the mtime day with `estimated` for an edit made outside it.
12. **`annotations.json`.** `{"version":1,"items":[...]}`, where each item has a `type`: `line`, `square`, `circle`, `stroke` or `text`. Limits: 5k items, 4 MiB. [ui]

## C. Graph
13. **Index.** Vertices; `E_tree` edges from `parent`; weighted `E_ref` edges from the body, excluding self-links and links that duplicate the parent edge; `refsIn`/`refsOut`; roots; depth by BFS from parentless notes. Source: `graphModel.ts#createGraphIndex`.
14. **Issues:** `missing-parent` (the parent reference does not resolve), `cycle`, and `disconnected`. (gap) `duplicate-alias` is defined but never raised.
15–19. Layered layout, large-graph mode, label tiers, the SVG canvas and annotations. [ui]

## D. Editing
20. **Create note.** The filename is slugified and de-duplicated. A title that is already taken gets a " 2" suffix. Commands: `create_note`, `create_notes`.
21. Autosave editor, which writes only when the content changed. [ui]
22. **Auto-create linked notes.** A newly typed `[[X]]` that resolves to nothing creates a loose note X. Code fences, `![[…]]`, media extensions, `#` and relative paths are excluded.
23. **Rename (title only).** Changing a title rewrites every body wikilink that resolved to the note into `[[New Title]]`, keeping any `|label`. The file itself is never renamed.
24. **Reparent.** Sets `parent`. The app refuses to make a note a child of one of its own descendants.
25. **Reference link.** Appends `[[target]]` to the source note's body.
26. **Delete to trash.** Files go to the OS trash, and body links to them become plain label text. Manifest and layout are updated. (gap) Children get `parent: null`, although the docs say they keep broken parents.
27–29. Copy/paste nodes, drag-and-drop import and undo history. [ui]

## E. Search and navigation
30. **Search.** BM25 over title (weight 3), path (1.4) and body (1), falling back to trigram fuzzy matching. Source: `web/searchIndex.ts`. [ui for results]
31. Workspaces and tabs, recent projects, and creating a project with `<slug>/notes/`, an empty manifest and an empty layout. [ui]

## F. Validation
32. **Validation status.** Reports missing frontmatter, title or parent, plus the graph issues. There is no auto-repair beyond items 4, 23 and 26.
33. **Live sync.** Polls every 1.5 s using a signature of `mtimeSecs:nanos:len`, then merges and reconciles dates.
34. **Size warnings.** Shown above 1000 notes, 50 MiB total, or 2 MiB for a single note. [ui]

## G. Agent integration
35. **Writing skill.** `skills/apex-notes-writing/SKILL.md` requires two-key frontmatter, hierarchy through `parent` only, and a sorted manifest. An agent following it ends with the report line `notes=N parentless=N connected=N manifest_missing=N manifest_extra=N fixes=[...]`.
36. **Hierarchy prompt.** A copyable prompt that bundles the SKILL.md for an outside agent to run. [ui]
37. `AGENTS.md` repository rules.

## H–I. Build and desktop only
Build scripts, macOS packaging, CI, native dialogs and self-update. [ui]

## Re-inventory notes (re-inventory 2026-10-04)
Read at the same commit, f449644a (#359). Corrections to the items above, then the items the list folded into others or missed; ADR 0021's Apex rows 43 to 55 disposition them.

- **Item 7.** Apex refuses a `notes` subfolder that is a symbolic link, with the message "Notes folder cannot be a symlink" (src-tauri/src/lib.rs:236-243, `read_workspace_blocking`).
- **Item 9.** Apex reads an unparseable layout.json as empty and replaces the file with only the patched keys on its next patch (lib.rs:1443-1453 `read_layout_file`, lib.rs:447-474 `write_layout_patch_blocking`).
- **Item 12.** Besides 5k items and 4 MiB, lib.rs:105-118 caps 4,096 points per stroke, 100,000 points in total, 120 characters per name, 20,000 characters of text per item and 200,000 in total.
- **Item 23.** Apex's rename rewrites body wikilinks only: `updateRenamedNoteLinks` (web/appShell.ts:3774-3805) keeps each note's frontmatter, so a child whose parent named the old title is left with a parent that no longer resolves.
- **Item 24.** Apex's reparent can rewrite the parent note as well as the child (findFallbackApex, web/appShell.ts:7497-7533): a parent with no usable hierarchy slot is given the first valid apex note as its own parent.
- **Item 30.** The index has four fields: title (3), path (1.4), searchText (1) and body (1) (web/searchIndex.ts:1-420).

New items:

- 43 path safety for workspace files. A symlinked notes/, manifest.json or layout.json is refused (lib.rs:236-243 for notes/).
- 44 workspace tabs (web/appShell.ts:1102-1222). [ui]
- 45 recent projects (appShell.ts:1346-1380). [ui]
- 46 project folder rename (appShell.ts:1058-1131). [ui]
- 47 project location chooser (appShell.ts:1382-1537). [ui]
- 48 paste plain text as a note (web/notes/noteComposer.ts:67-96). [ui]
- 49 editor link rendering (appShell.ts:179-298). [ui]
- 50 per-line date labels and the note date header (web/ui/dateDecorations.ts:1-281). [ui]
- 51 date tracking that never blocks Markdown. A note edit lands even when dates.json cannot follow it (README.md).
- 52 search result ranking layer (web/searchResults.ts:1-255).
- 53 rope connect gesture (appShell.ts:5883-5920). [ui]
- 54 hub and PageRank scoring (web/labelVisibility.ts:19-234). [ui]
- 55 workspace chrome (appShell.ts:443-637). [ui]
