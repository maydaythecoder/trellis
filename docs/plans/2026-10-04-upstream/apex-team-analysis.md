## Product Manager

- **Scope.** The re-inventory has 55 rows (APX-01 to APX-55), read from apex-notes `f449644a`. ADR 0021 and ADR 0022 cite the same commit, so upstream has not moved. The work is measured against the existing inventory and ADR rows.
- **Result:**
  - two ports: APX-43 path safety and APX-51 date tracking;
  - one docs record covering rows 43 to 55 and five inventory corrections;
  - three owner questions: the search ranking layer, the writing skill, and model writes into the vault.
- **No desktop-ui row gets a build ticket.** ADR 0021 [ui] rows and the owner's answer on #70 keep them excluded. If the desktop track adopts a surface, that needs an ADR amending ADR 0021, and APEX-03 says so in one line.
- **What users get today without these tickets:**
  - With a symlinked `notes/`, trellis silently reads a different set of files than Apex.
  - `trellis vault new`, `rm` and `check --fix` rewrite the target of a `manifest.json` or `layout.json` link outside the vault.
  - A vault edit exits non-zero after its note write has landed, whenever `dates.json` cannot follow.
  - ADR 0021 stops at row 42, and row 35 still reads as built.
- **Priority:**
  1. APEX-01 and APEX-02: ready, single area plus their ADR line.
  2. APEX-03: docs, can land now; its rows say "landing" until 01 and 02 merge.
  3. APEX-04, APEX-07 and APEX-08: parked on the owner. APEX-05 waits on APEX-04.
- **Deliberately not raised, because the exclusions stand:**
  - a CLI import (APX-28, APX-48);
  - live sync (kept excluded on #68);
  - auto-created linked notes (#61);
  - date labels and PageRank as CLI output (APX-50, APX-54).

## Principal System Architect

- **Crate boundaries hold.** No ticket touches a `Cargo.toml`. `trellis-vault` gains neither `trellis-model` nor `trellis-core`, not even as a dev-dependency. The Windows-skipping symlink test helper is written inline.
- **Public signatures are kept on purpose:**
  - `notes_root` stays as it is: it is re-exported and called from `vault_cmd.rs:346`.
  - `dates::after_edit` keeps its `Result<Option<String>, _>` signature.
  - `Index::search` is unchanged.
  - As a result, APEX-01 and APEX-02 need no `trellis-cli` edit, and the Node-generated Apex vectors (`ranking_matches_apex_vectors`, `graph_matches_apex_vectors`) stay valid.
- **The run gate changes in one direction.** A vault with a symlinked `notes/` used to be read from the parent directory and is now refused. This is the one new way a vault the gate accepted can be refused. It is recorded in ADR 0022 §1. The three goldens are unaffected, because the fixture has no link.
- **Search ranking is a separate function in `trellis-vault`.** Under the recommended option B, `search_notes` keeps ADR 0024 §6's index ranking. What the model is shown stays byte-identical, and only the human's CLI moves to Apex's search-box order.
- **APEX-08 option A extends the CLAUDE.md invariant** "Model writes never land in `.trellis/` or `.git/`", so it carries its own ADR (0046, renumbered if taken). Option B is documentation only.
- **The grounding contract is untouched.** No ticket creates a `Verdict`, skips a check, multiplies a budget, or reads a trail as trusted.

## Senior Full-Stack Engineer

- **APEX-01.**
  - Add `symlink_metadata` checks in `Vault::open` (after `notes_root`), in the gate's parse in `validate.rs`, and before any write in the edit session (`Session::open` or `sync_manifest`/`prune_layout`).
  - Error texts use Apex's wording, "cannot be a symlink".
  - `dates_cmd` already calls `Vault::open` before `dates::load`, so the `notes/` refusal covers `vault dates --seed`.
  - `compile_for_run` reads `dates.json` from the fallback root before `compile` refuses. That is a read of a file that is then discarded, and nothing is written.
  - Tests go in `tests/parity.rs` with `copy_vault()`. Each test returns early with `skipped:` when the host cannot create a link.
- **APEX-02.**
  - Mirror `refresh`'s per-note handling (`dates.rs:674-726`): keep the previous entry on failure, collect one message per failing note, and turn a `save` failure into a message.
  - The workspace-limit test needs a document of about 200,000 lines, roughly 14 MB compact and under the 16 MiB cap. The 20,001-note route reaches the same branch but is slower.
- **APEX-04.**
  - Port `buildSearchResults`, `scoreNote`, `scoreField`, `subsequenceScore` and `indexScore` in `f64`.
  - `Index` needs an options variant: trigrams always merged, minimum 0.12, limit max(4 x limit, note count).
  - Reuse ADR 0022 §4's `localeCompare` approximation.
  - Generate vectors with Node from the unmodified `.ts`, as ADR 0022 "Evidence" did.
- **APEX-05.**
  - Factor the printing in `search` into a function that returns lines.
  - The discriminating test is a subsequence-only query (`rtp` against the title `Retry policy`). The index returns nothing for it and the layer returns the note.
- **APEX-07.** One unit test in `skills.rs`, holding Apex's two frontmatter lines verbatim (MIT, attributed).
- **Effort:** 01 m, 02 m, 03 s, 04 m, 05 s, 07 s, 08 m.

## QA / Security Engineer

- **APEX-01 closes a write-through-symlink path.** Today a `manifest.json` or `layout.json` linked outside the vault has its target rewritten by `vault new`, `rm` and `check --fix`, and the run gate reads the manifest through the link.
  - What remains: a check-then-write race between open and write. Apex has the same race (`reject_symlink`, then `fs::write`). Not closed, and stated as such.
  - Note paths come only from the walk, which skips links, or from a root-level slug written with `create_new`, so no `..` traversal is reachable.
- **Windows evidence is weak.** The symlink tests skip on Windows hosts without Developer Mode, and `verify-tests.sh` counts a skip as a pass, so the Linux CI job is the real evidence. Reviewers should check that job's output.
- **APEX-01's fourth test kills #246's surviving mutant** `dates.rs:469: replace is_symlink -> bool with false`. Nothing tests that refusal today.
- **APEX-02 changes the exit status.** A date failure after a landed note edit now exits 0 instead of 1. The signal moves to a printed line containing `dates.json left unchanged`, and the tests assert `dates.json` bytes are unchanged.
- **APEX-04 option A would change prompt input** for every run with `search_notes` enabled. Option B leaves that output byte-identical, which `a_search_names_matching_notes_and_says_what_they_are` holds.
- **APEX-08 finding.** Through `write_file` or `edit` into `notes/`, the model can write its own future context unless `deny_write` names the vault. This is the exact concern of #30, and the `notes.rs` module comment ("a run can edit it") contradicts ADR 0022 §3.
- **Every Verify line passes G6:** only `cargo`, `git` and `scripts/` prefixes, no `..`, no pipes, redirects or substitution. No decision stems appear in any title or body, including the question sections, because `trellis-tickets` exempts only `## Decision`.

## Next Steps & Action Plan

1. File APEX-01 and APEX-02 now. Both are ready: no blockers, Verify is executable, and each has one code area plus its ADR 0022 line.
2. File APEX-03 now. Its rows 43 and 51 say "landing, under the ADR 0022 §1/§5 amendment", and are flipped to built after APEX-01 and APEX-02 merge, as ADR 0021's "landing" convention requires.
3. File APEX-04, APEX-07 and APEX-08 with the `parked` label and their `## Question for the owner`. File APEX-05 as `triage:blocked` on APEX-04, and drop it if the owner answers C.
4. After the owner answers: for APEX-08 option A, write ADR 0048 first, since it changes a CLAUDE.md invariant. APEX-07 option A is one test and one row amendment.
5. Hand the desktop-ui rows (15-19, 21, 27-29, 38-42, 44-50, 53-55) to the desktop track as input. Building any of them needs an ADR amending ADR 0021 (#70).
