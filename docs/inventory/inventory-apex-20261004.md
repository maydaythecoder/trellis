# apex-notes capability inventory (re-read at f449644a)

- **Upstream:** `boriemannetje/apex-notes`, commit `f449644a459b67c3b5f2d05ea676a48c1f63fe0e`, 2026-09-09 +0200, Boris de Wit, "Preserve daily labels in multiline links and bound overlap checks". App version 0.1.9 (`package.json`, `src-tauri/tauri.conf.json`).
- **Licence:** MIT (`LICENSE`, copyright 2026 Boris de Wit).
- **Re-read:** 2026-10-04. This is the commit `docs/inventory/inventory-apex.md`, ADR 0021 and ADR 0022 already cite, so the upstream code is unchanged. What changed is how much of it was read, and the TRELLIS status of some rows.

## Method

- **Source.** A local clone (shallow, one commit), read file by file. Nothing was run: no `npm`, no `cargo`, no TypeScript vectors. The earlier inventory was made through `gh api`, also without running anything.
- **Compared against:**
  - `docs/inventory/inventory-apex.md`;
  - the Apex rows of ADR 0021 and its amendments;
  - ADR 0022;
  - `crates/trellis-vault` (all modules; `validate.rs` and `compile.rs` grepped rather than read whole);
  - `crates/trellis-cli/src/vault_cmd.rs`;
  - `crates/trellis-loop/src/notes.rs`;
  - the relevant lines of `context_files.rs`, `run_cmd.rs` and `registry.rs`.
- **Read in full or nearly in full:**
  - `src-tauri/src/lib.rs` lines 1-1599 and 2465-2500 (tests by name only);
  - every module under `web/notes/`, `web/persistence/` and `web/state/`;
  - `web/searchIndex.ts`, `web/searchResults.ts`, `web/noteRefs.ts`, `web/graphModel.ts`, `web/graphConfig.ts`, `web/graphLargeMode.ts`, `web/graphWheelZoom.ts`, `web/nodeSizing.ts`, `web/popoverPositioning.ts`, `web/recentProjects.ts`, `web/ui/dateDecorations.ts`;
  - `README.md`, `AGENTS.md`, `CONTRIBUTING.md`, `SECURITY.md`, `manualtest.md`, `docs/release-*.md`, `skills/apex-notes-writing/SKILL.md`;
  - both workflows, both scripts, the Tauri config and capabilities, and the `notes/` sample data.
- **Read by function name or exports only:**
  - `web/appShell.ts` lines 330-1057, 2579-3012, 3305-3442, 4039-5195, 5563-6802 and 6931-7221 (about 3,500 of 8,335 lines);
  - `web/graphLayout.ts` 60-566, `web/graphPositioning.ts` 100-234, `web/labelVisibility.ts` 420-827, `web/annotations.ts` 80-213.
- **Not read:**
  - test bodies (only the e2e test titles);
  - `web/index.html` beyond one grep, `web/styles.css`, `web/icons.ts`, `web/ui/domElements.ts`;
  - the `.github` issue and pull-request templates, `CODEOWNERS`;
  - `package-lock.json`, `src-tauri/Cargo.lock` and the binary assets.
- **Head commit.** The clone has one commit, so its diff is not visible. Its subject line matches `web/appShell.ts:5479-5480` (a Markdown link spanning lines is left raw so each line keeps its daily date label) and `5507-5515` (wiki-link matches are checked against Markdown-link ranges with a moving index). Nothing more is claimed about what it changed.

### Markers (in each row's notes)

- **NEW:** not an item of `inventory-apex.md`. This includes behaviour the earlier pass folded into a broader item and that is listed on its own here.
- **CHANGED:** the inventory's description needed correcting or extending, or the TRELLIS status differs from what the ADR 0021 row first said (for example 22, W-A then excluded under #61).
- **UNCHANGED:** both the description and the status hold.

### Ids

- APX-01 to APX-37 are the inventory's item numbers.
- The inventory grouped 15-19 and 27-29. This re-read splits them:
  - 15 layered layout;
  - 16 large-graph mode;
  - 17 label tiers;
  - 18 graph canvas and navigation;
  - 19 canvas annotation drawing;
  - 27 copy and paste, 28 drop import, 29 undo.
- ADR 0021 names "38-42 build, packaging, native, self-update", which the inventory did not number. They are assigned here as 38 build, 39 macOS packaging, 40 CI and release, 41 native dialogs and links, 42 self-update.
- New rows start at APX-43.

### TRELLIS status

- **built:** behaviour present in TRELLIS.
- **partial:** part present, with the gap named.
- **missing:** the behaviour is not in TRELLIS and no ADR 0021 row excludes it. Used where the behaviour bears on the vault's files or what a user sees from them (50, 52).
- **excluded:** an ADR 0021 row names it, or it is a UI-only surface covered by the owner decision of 2026-09-28 (#70: "Desktop and web interfaces stay excluded under brief Part 1"). The clause is cited in each row.

### Disposition (values from the task schema, not from ADR 0021)

- **already-built:** status built.
- **port:** missing or partial behaviour that belongs in `trellis-vault` or `vault_cmd.rs`.
- **desktop-ui:** belongs to a desktop or editor front end. Excluded from TRELLIS under #70, and reachable only by an outside front end driving `trellis rpc`, whose writes still go through `trellis vault`.
- **drive-via-runtime:** Apex hands a prompt to an outside agent; TRELLIS would run it through its own loop. Assigned to no row. The only candidate, 36, would have the model write vault notes, and ADR 0022 §3 amendment (#30) decided against a note-write tool.
- **owner-decision:** needs, or would reverse, an owner call.

## Counts

- **Rows:** 55.
- **Markers:** 32 UNCHANGED, 10 CHANGED, 13 NEW.
- **TRELLIS status:** 23 built, 3 partial, 2 missing, 27 excluded.
- **Disposition:** 23 already-built, 2 port, 3 owner-decision, 27 desktop-ui.

## Features

Paths in the upstream column are relative to the apex-notes root. Paths in the TRELLIS column are relative to `crates/` unless an ADR is named. The fuller evidence lists are in the structured `features` array this table was written from.

| Id | Capability | Marker | TRELLIS | Disposition | Upstream evidence | TRELLIS evidence |
|---|---|---|---|---|---|---|
| APX-01 | Frontmatter parsing (two meaningful keys, regex lines, quotes stripped) | UNCHANGED | built | already-built | `web/notes/noteParser.ts:87-151`; `AGENTS.md:5-21` | `trellis-vault/src/reader.rs:108-170, 212-258`; ADR 0022 §1 |
| APX-02 | Title fallback (title, first H1, basename) | UNCHANGED | built | already-built | `web/notes/noteParser.ts:45-56` | `trellis-vault/src/reader.rs:172-240` |
| APX-03 | Parent edge and the four null forms | UNCHANGED | built | already-built | `web/notes/noteParser.ts:43, 82-85`; `web/graphModel.ts:119, 398-412`; `web/notes/noteComposer.ts:25-42` | `trellis-vault/src/reader.rs:164-170`; `graph.rs:108-117`; `edit.rs:97-106` |
| APX-04 | Two-key rewrite (composeRaw), legacy keys dropped | UNCHANGED | built | already-built | `web/notes/noteComposer.ts:15-23, 98-100`; `src-tauri/src/lib.rs:1488-1496` | `trellis-vault/src/edit.rs:26-56, 121-129`; `tests/parity.rs` fix tests |
| APX-05 | Body wikilinks and weighted reference edges | UNCHANGED | built | already-built | `web/noteRefs.ts:2-30`; `web/graphModel.ts:211-229, 431-442, 465-494` | `trellis-vault/src/refs.rs:91-128, 159-178`; `graph.rs:214-311` |
| APX-06 | Implicit aliases and resolution order | UNCHANGED | built | already-built | `web/noteRefs.ts:37-70`; `web/graphModel.ts:142-164, 287-307` | `trellis-vault/src/graph.rs:30-118`; `refs.rs:15-46, 136-158` |
| APX-07 | Notes root and file walk (symlinked notes/ refused in Apex) | CHANGED | built | already-built | `src-tauri/src/lib.rs:236-243, 1267-1304, 1348-1353` | `trellis-vault/src/reader.rs:14-66` |
| APX-08 | manifest.json (written, never read) | UNCHANGED | built | already-built | `src-tauri/src/lib.rs:426-445, 1434-1441, 1455-1474`; `web/appShell.ts:8233-8240` | `trellis-vault/src/validate.rs:340-400`; `edit.rs:131-151, 184-197` |
| APX-09 | layout.json positions | CHANGED | built | already-built | `src-tauri/src/lib.rs:447-474, 1443-1453`; `web/appShell.ts:7953-8018` | `trellis-vault/src/edit.rs:156-197`; ADR 0021 row 9 |
| APX-10 | dates.json schema, validation, atomic write | UNCHANGED | built | already-built | `src-tauri/src/lib.rs:51-103, 512-553, 848-1045, 1498-1569`; `web/notes/noteDates.ts:183-310` | `trellis-vault/src/dates.rs:21-130, 384-620`; ADR 0022 §5 |
| APX-11 | Seeding and reconciling dates | UNCHANGED | built | already-built | `web/notes/noteDates.ts:58-140, 312-337`; `web/appShell.ts:1949-1991, 2054-2103` | `trellis-vault/src/dates.rs:182-382, 674-800`; `trellis-cli/src/vault_cmd.rs:362-400` |
| APX-12 | annotations.json (canvas drawings) | CHANGED | excluded | desktop-ui | `src-tauri/src/lib.rs:105-195, 476-510, 1047-1241`; `web/annotations.ts:1-80` | ADR 0021 row 12 ("editing is excluded [ui]"); preserved, `tests/parity.rs` sidecars_survive_* |
| APX-13 | Graph index | UNCHANGED | built | already-built | `web/graphModel.ts:121-285, 309-329, 376-380` | `trellis-vault/src/graph.rs:188-356`; `trellis-cli/src/vault_cmd.rs:245-308` |
| APX-14 | Graph issues | UNCHANGED | built | already-built | `web/graphModel.ts:3-8, 331-358, 496-536`; `web/notes/noteValidation.ts:55-69` | `trellis-vault/src/graph.rs:84-104, 120-186, 214-310, 358-397`; ADR 0022 §2 |
| APX-15 | Layered graph layout and grid mode | UNCHANGED | excluded | desktop-ui | `web/graphLayout.ts:1-566`; `web/graphPositioning.ts:1-234` | ADR 0021 row 15-19 "excluded [ui]" and itemised list |
| APX-16 | Large-graph mode | UNCHANGED | excluded | desktop-ui | `web/graphConfig.ts:1-6`; `web/graphLargeMode.ts:1-108` | ADR 0021 itemised "Large-graph mode: excluded, [ui]" |
| APX-17 | Label tiers by zoom | UNCHANGED | excluded | desktop-ui | `web/labelVisibility.ts:9-17, 239-420`; `web/appShell.ts:5354-5468` | ADR 0021 itemised "Label tiers: excluded, [ui]" |
| APX-18 | Graph canvas and navigation | UNCHANGED | excluded | desktop-ui | `web/appShell.ts:4039-4296, 5563-5822, 6403-7220`; `web/nodeSizing.ts:1-25`; `web/graphWheelZoom.ts:1-59` | ADR 0021 row 15-19 and itemised list |
| APX-19 | Canvas annotation drawing | UNCHANGED | excluded | desktop-ui | `web/annotations.ts:1-213`; `web/appShell.ts:5927-6401` | ADR 0021 itemised "Drawing freehand annotations on the canvas: excluded, [ui]" |
| APX-20 | Create note | UNCHANGED | built | already-built | `web/notes/noteComposer.ts:25-65`; `src-tauri/src/lib.rs:378-424`; `web/appShell.ts:7222-7330, 8062-8231` | `trellis-vault/src/edit.rs:199-274` |
| APX-21 | Autosave editor | UNCHANGED | excluded | desktop-ui | `web/appShell.ts:378-442, 3443-3516, 3637-3772` | ADR 0021 row "21, 27-29 ... excluded [ui]" |
| APX-22 | Auto-create linked notes | CHANGED | excluded | desktop-ui | `web/appShell.ts:3890-4037` | ADR 0021 row 20, 22-26 amendment (#61); ADR 0022 §7 |
| APX-23 | Retitle with link rewrite | CHANGED | built | already-built | `web/appShell.ts:3774-3805, 3849-3888` | `trellis-vault/src/edit.rs:344-422`; ADR 0022 §3 |
| APX-24 | Reparent (and Apex's parent-rewrite plan) | CHANGED | built | already-built | `web/appShell.ts:3461-3503, 7332-7404, 7497-7533` | `trellis-vault/src/edit.rs:277-307`; ADR 0022 §3 |
| APX-25 | Reference link | UNCHANGED | built | already-built | `web/appShell.ts:7406-7476` | `trellis-vault/src/edit.rs:310-341` |
| APX-26 | Delete (Apex: OS trash) | UNCHANGED | built | already-built | `src-tauri/src/lib.rs:555-579`; `web/appShell.ts:3518-3635, 3807-3835` | `trellis-vault/src/edit.rs:425-462`; ADR 0021 row 20 |
| APX-27 | Copy, cut and paste nodes | UNCHANGED | excluded | desktop-ui | `web/appShell.ts:7613-7748, 7769-7865` | ADR 0021 row 21, 27-29 |
| APX-28 | Drag-and-drop Markdown import | UNCHANGED | excluded | desktop-ui | `web/appShell.ts:7867-7951`; `web/notes/noteComposer.ts:67-96` | ADR 0021 row 21, 27-29 |
| APX-29 | Workspace undo and redo | UNCHANGED | excluded | desktop-ui | `web/appShell.ts:2257-2570`; `web/state/historyBudget.ts:1-93` | ADR 0021 row 21, 27-29 |
| APX-30 | Search (BM25, trigram fallback) | CHANGED | built | already-built | `web/searchIndex.ts:1-420`; `web/appShell.ts:2186-2240` | `trellis-vault/src/search.rs:1-381`; `trellis-loop/src/notes.rs:1-80`; ADR 0022 §4; ADR 0024 §6 |
| APX-31 | Project creation | CHANGED | built | already-built | `src-tauri/src/lib.rs:290-313, 1355-1370, 2479-2496` | `trellis-vault/src/edit.rs:465-488`; ADR 0022 §3 |
| APX-32 | Validation status | UNCHANGED | built | already-built | `web/notes/noteValidation.ts:25-69`; `web/appShell.ts:8020-8060` | `trellis-vault/src/edit.rs:494-595`; `trellis-cli/src/vault_cmd.rs:186-226` |
| APX-33 | Live folder sync | UNCHANGED | excluded | desktop-ui | `src-tauri/src/lib.rs:245-288`; `web/persistence/liveSync.ts:1-69`; `web/appShell.ts:1694-1883` | ADR 0021 row 32-34 "live sync excluded (no long-lived process)" |
| APX-34 | Size warnings | UNCHANGED | built | already-built | `web/appShell.ts:133-135, 2122-2247` | `trellis-vault/src/edit.rs:488-490, 542-595` |
| APX-35 | Writing skill (SKILL.md) | CHANGED | partial | owner-decision | `skills/apex-notes-writing/SKILL.md:1-84` | ADR 0022 §6; report line `trellis-vault/src/edit.rs:542-556` |
| APX-36 | Hierarchy prompt for an outside agent | UNCHANGED | excluded | owner-decision | `web/appShell.ts:100-110, 3205-3265` | ADR 0021 row 35-37 "the prompt is excluded [ui]"; ADR 0022 §3 amendment (#30) |
| APX-37 | AGENTS.md repository rules | CHANGED | built | already-built | `AGENTS.md:1-60` | `trellis-cli/src/context_files.rs:25`; ADR 0025 |
| APX-38 | Web build scripts | UNCHANGED | excluded | desktop-ui | `package.json:1-36`; `scripts/build-web.mjs:1-38` | ADR 0021 row 38-42 "excluded [ui/desktop]" |
| APX-39 | macOS packaging | UNCHANGED | excluded | desktop-ui | `scripts/package-macos-release.sh:1-81`; `docs/release-checklist.md:1-49` | ADR 0021 row 38-42 |
| APX-40 | CI and release automation | UNCHANGED | excluded | desktop-ui | `.github/workflows/pr-safety.yml:1-73`; `.github/workflows/release-latest.yml:1-96` | ADR 0021 row 38-42 |
| APX-41 | Native dialogs and external links | UNCHANGED | excluded | desktop-ui | `web/persistence/nativeWorkspaceAdapter.ts:1-45`; `src-tauri/capabilities/default.json:1-17` | ADR 0021 row 38-42 |
| APX-42 | Self-update | UNCHANGED | excluded | desktop-ui | `web/persistence/appUpdates.ts:1-139`; `src-tauri/src/lib.rs:627-713` | ADR 0021 row 38-42 |
| APX-43 | Path safety for workspace files | NEW | partial | port | `src-tauri/src/lib.rs:236-243, 866-915, 1372-1432, 1571-1580` | `trellis-vault/src/dates.rs:468-500`; gap at `edit.rs:142-183`, `reader.rs:14-23` |
| APX-44 | Workspace tabs | NEW | excluded | desktop-ui | `web/appShell.ts:1102-1222, 2571-2910`; `web/state/workspaceStore.ts:1-122` | ADR 0021 row 31 "tabs excluded [ui]" |
| APX-45 | Recent projects | NEW | excluded | desktop-ui | `src-tauri/src/lib.rs:581-625, 743-803`; `web/recentProjects.ts:1-124` | ADR 0021 owner decisions 2026-09-28 (#70) |
| APX-46 | Project folder rename | NEW | excluded | desktop-ui | `src-tauri/src/lib.rs:326-360`; `web/appShell.ts:1058-1131` | ADR 0021 #70 |
| APX-47 | Project location chooser | NEW | excluded | desktop-ui | `src-tauri/src/lib.rs:315-324`; `web/appShell.ts:1382-1537` | ADR 0021 #70 |
| APX-48 | Paste plain text as a note | NEW | excluded | desktop-ui | `web/notes/noteComposer.ts:67-96`; `web/appShell.ts:7686-7767` | ADR 0021 row 21, 27-29 |
| APX-49 | Editor link rendering | NEW | excluded | desktop-ui | `web/appShell.ts:179-298, 5470-5544` | ADR 0021 row 21 and #70 |
| APX-50 | Per-line date labels and note date header | NEW | missing | desktop-ui | `web/notes/noteDates.ts:142-177`; `web/ui/dateDecorations.ts:1-281`; `web/appShell.ts:2105-2120` | no row; nearest `trellis-vault/src/compile.rs:89-102` (ADR 0022 §7) |
| APX-51 | Date tracking that never blocks Markdown | NEW | partial | port | `web/appShell.ts:1949-2052, 2054-2103`; `README.md:69-71` | `trellis-vault/src/dates.rs:674-800`; `trellis-cli/src/vault_cmd.rs:170-196` |
| APX-52 | Search result ranking layer | NEW | missing | owner-decision | `web/searchResults.ts:1-255` | ADR 0022 §4 "not reproduced" |
| APX-53 | Rope connect gesture | NEW | excluded | desktop-ui | `web/appShell.ts:3192-3203, 6603-6660, 6803-6930` | ADR 0021 row 15-19 and #70 |
| APX-54 | Hub and PageRank scoring | NEW | excluded | desktop-ui | `web/labelVisibility.ts:19-234` | ADR 0021 itemised "Label tiers: excluded, [ui]" |
| APX-55 | Workspace chrome (pane resize, fullscreen, help, launch, status) | NEW | excluded | desktop-ui | `web/appShell.ts:443-637, 770-1056, 8253-8327`; `web/icons.ts:1-63` | ADR 0021 #70 |

## Row notes for CHANGED and NEW rows

- **APX-07.** Apex refuses a symlinked `notes/` folder (`lib.rs:240`). TRELLIS does not follow it, and reads the opened directory itself instead, so the two read different files for that layout.
- **APX-09.** Apex reads an unparseable `layout.json` as `{}`, and the next patch replaces it with only the patched keys. TRELLIS leaves it untouched and says so. Apex writes positions for new or moved notes on most edits; TRELLIS never adds any (row 9, "never interpreted").
- **APX-12.** Exact limits:
  - 5,000 items;
  - 4,096 points per stroke and 100,000 stroke points in all;
  - names of at most 120 characters;
  - text of at most 20,000 characters per item and 200,000 in all;
  - 4 MiB in total.
  IDs must be unique and geometry finite. A degenerate shape is rejected. A file that fails validation, or is a newer version, is never overwritten, and a symlinked file is refused. Validation is shared between TypeScript and Rust through `tests/fixtures/annotations-parity.json`.
- **APX-22.** First assigned to W-A. Excluded on 2026-09-29 (#61) because a command line has no "newly typed" event; `trellis vault new` is the explicit form.
- **APX-23.** Apex rewrites body links only. A child whose `parent` named the old title is left with a broken parent. TRELLIS rewrites that parent too, so here it does more than Apex.
- **APX-24.** When the parent has no usable hierarchy slot, Apex's connection plan also rewrites the parent, putting it under the first valid parentless root (`findFallbackApex`). ADR 0022 §3 declines this; TRELLIS rewrites only the child.
- **APX-30.** The code indexes four fields, not the three the inventory lists. The model tools are `search_notes` and `read_note` under ADR 0024 §6; ADR 0021 row 30 credits W-B.
- **APX-31.** Tabs, recent projects, folder rename and the location chooser are now 44-47. Apex slugifies the folder name (ASCII only), de-duplicates it and creates `<slug>/notes/`. TRELLIS takes the directory as given and refuses one that already has a `manifest.json` or `layout.json`.
- **APX-35.** ADR 0021 row 35-37 said W-A ships the contract as a built-in skill. ADR 0022 §6 does not, and invites the owner to override.
- **APX-37.** TRELLIS ships no Apex rules of its own. The contract reaches the model only when the target repository's own `AGENTS.md` carries it.
- **APX-43.** What Apex does:
  - every note-path component goes through `safe_child_path`: only normal components, symlinks refused, the canonical parent required inside the notes root;
  - only `.md` paths may be written;
  - the four sidecar files are refused when they are symlinks;
  - sidecar reads are bounded.

  What TRELLIS does:
  - refuses a symlinked `dates.json` and bounds its read;
  - its walk skips symlinked entries;
  - `edit.rs` `sync_manifest` and `prune_layout`, and `validate.rs` `manifest_state`, read and write `manifest.json` and `layout.json` through a symlink.

  No `..` traversal was found reachable: TRELLIS writes paths from its own walk, or a slug at the root.
- **APX-44 to APX-47.** Folded into inventory item 31 before. Apex can rename the project folder; the inventory's "no way to rename or move a file" still holds for notes.
- **APX-48.** The title comes from the frontmatter title, else the first H1, else the first six words of the text (at most 56 characters), with code fences and punctuation removed. `titleFromText` is a pure function.
- **APX-49.** The head commit's subject matches this code; see Method.
- **APX-50.** Labels read "D Mon YYYY", "≈ D Mon YYYY" when estimated, or "Date unavailable". Consecutive non-blank lines with the same day share one label. `dateGroups` and `formatDateStamp` are pure functions that no TRELLIS feature uses.
- **APX-51.** Apex pauses date tracking (per note or per workspace), keeps the sidecar, and reports the Markdown write as safe. In TRELLIS:
  - `after_edit` stops at the first note that fails, so no touched note's dates are saved;
  - `print_outcome` then exits non-zero although the note write has landed.

  Both tools leave the sidecar intact.
- **APX-52.** Apex's search box adds substring, prefix, all-token and subsequence boosts on top of the index (title 120, filename 86, path 58, searchText 24). `trellis vault search` and Apex can therefore rank the same query differently.
- **APX-53.** The gesture behind 24 and 25. It reparents when the source note's hierarchy is incomplete or there are several parentless roots; otherwise it appends a body link.
- **APX-54.** Feeds only label visibility in Apex. It is a pure function of the graph.

## Read, and not a capability at this commit

- `notes/` holds neutral sample data: mission, project goal, a device roadmap ("supporting opening full codebases as apex notes", multimedia), and a UI styleguide.
- `notes/supportmedia.md` is a plan for attachments modelled on Obsidian. None of it is built.
- `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md` and `SECURITY.md` are project policy.
- The issue templates and `CODEOWNERS` were not read.

## Open questions

1. **#30 and the generic write tools.** `trellis-loop` has no vault-specific write refusal. `run_cmd.rs:821-835` builds the run's deny list only from `.trellis/governance/` rules and `[governance] deny_write`, with no default entry for `[context] vault`. Whether #30 meant to refuse generic `edit` and `write` calls to vault notes is not stated.
2. **APX-33.** The exclusion reason ("no long-lived process") predates `trellis work --every` (ADR 0020).
3. **APX-43.** Port symlink refusal for `manifest.json` and `layout.json`, and refuse a symlinked `notes/` folder as Apex does, or accept the divergence?
4. **APX-51.** Should an `after_edit` failure be a warning that does not change the exit status? Should one failing note stop the date update for every touched note?
5. **APX-52.** ADR 0022 §4 declines the `searchResults.ts` layer without citing an exclusion clause.
6. **APX-35.** ADR 0022 §6 invites the owner to override.
7. **APX-50 and APX-54.** `dateGroups` and the hub/PageRank scoring are pure functions; no use outside the UI is assigned.
8. **APX-28 and APX-48.** A command-line import (`vault new` from a file, with Apex's title derivation) is unassigned.
9. **Documentation drift.**
   - ADR 0021 row 30 credits `search_notes` to W-B, while `notes.rs` cites ADR 0024 §6.
   - ADR 0021 row 35-37 says W-A ships the skill, while ADR 0022 §6 says it does not.
10. **Upstream gaps carried forward.**
    - `manifest.json` is never read; `AGENTS.md`'s "seed preview" has no code.
    - The delete dialog says children keep broken parents, but the code sets `parent: null`.
    - `disconnected` and `duplicate-alias`, as Apex defines them, never fire.
