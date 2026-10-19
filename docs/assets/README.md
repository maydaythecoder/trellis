# Pictures, and how each one was made

Every image under `docs/assets/` was produced from this repository's own
binaries against real repositories, on 2026-10-19 from `dev` at `062f3e2`
(`trellis 0.1.0`, rustc 1.93.0, go 1.24.7, Linux x86-64). Nothing is mocked
up. Where a model appears, it is the `scripted:` model, and the caption says
so.

## The repositories shown

| Repository | What it is | Why it |
|---|---|---|
| poetry 2.5.0 | `python-poetry/poetry` at tag `2.5.0`, cloned with its history | the benchmark corpus of `BENCHMARK_AND_EVAL.md`; every figure in the pictures matches the figures in the documents (3,735 definitions, 451 files, 5,908 resolved) |
| trellis-demo | `qa/fixtures/trellis-demo.bundle`, the fixture the guard and behaviour suites run against | small enough to read in a screenshot, and it carries one deliberate defect: `describe` is annotated `-> int` and returns a string |

The `scripted:` model (`crates/trellis-loop/src/scripted.rs`) is a real
`Model` whose decisions are written down in advance. It drives the real
loop, real files, real mypy and real rollback; only the judgement is stubbed.
The two scripts used are `qa/fixtures/scripted-annotate.json` (edit, run the
typecheck, done) and `qa/fixtures/scripted-done-after-edit.json` (edit, then
claim success with no check). No result from a scripted run is a model floor
(S7), and none is presented as one.

## Terminal recordings

`cli.gif` and `tui.gif` are asciicast v2 recordings rendered with
[agg](https://github.com/asciinema/agg). The commands were typed into a real
pty at a human cadence; the output is whatever came back. `RUST_LOG` was set
to the empty string so that the tracing lines, which wrap badly at 104
columns, stayed out of the frame; nothing else was filtered. The theme is the
desktop's dark palette from `desktop/ui/style.css`. Playback is at 1.25x
with idle time capped at 2.5 s.

`cli.gif` runs, in order: `trellis index` and `trellis radius choose_for` in
poetry 2.5.0; then in trellis-demo, a scripted run that converges and leaves
`app/calc.py` modified, `git status` showing it, a commit accepting it, a
scripted run that edits and claims success without a check, and `git status`
showing the tree restored.

`tui.gif` is `trellis tui --model scripted:../scripts/annotate.json` on
trellis-demo with the task typed in and run: the Go interface (`tui/`),
handed the terminal by `trellis` because `trellis-tui` was installed beside
it. The pty answered the cursor-position query the interface sends at start,
as a terminal would; nothing else was answered.

## Desktop screenshots

The page is the desktop's own, served the way `trellis --web` serves it:
`desktop/src/web.rs` over the core in `desktop/src/commands.rs`, both
compiled unchanged, with only `main.rs` (the Tauri window) replaced by a
`main` that calls `web::serve` directly, because the machine the pictures
were made on has no WebKitGTK to build the window against. Headless Chromium
opened the URL the server printed, at 1280x820 and device scale 2, in the
light and the dark system theme, and each section was cropped to its own
element. Every byte on the page came from `trellis rpc` and `trellis --json
log` through the same reducer, the same four commands and the same events
the window uses. The Run, approval and Trails pictures come from a run
started from the page's own composer, with approvals set to ask.

| File | Section, repository |
|---|---|
| `desktop-approval-{light,dark}.png` | the approval dialog, trellis-demo, before the scripted edit was applied |
| `desktop-run-{light,dark}.png` | Run, trellis-demo, after the scripted annotate run converged |
| `desktop-trails-{light,dark}.png` | Trails, trellis-demo, with the newest run selected |
| `desktop-atlas-{light,dark}.png` | Atlas, poetry 2.5.0, Churn lens, commits in the last 90 days |
| `desktop-radius-{light,dark}.png` | the Symbol radius pane of Codebase, poetry 2.5.0, `choose_for` |

## Diagrams

`run-*.svg`, `radius-*.svg` and `crates-*.svg` are hand-written SVG in the
desktop's palette, one file per theme, chosen by the `<picture>` element in
the README. The radius figures are `trellis radius choose_for` on poetry
2.5.0. The crate rows follow the `[dependencies]` tables as
`docs/ARCHITECTURE.md` reads them, plus the two `trellis rpc` clients.

## Regenerating

There is no script here that regenerates these, because the harness that
drove the page is not part of the product and should not be. Re-make them by
hand against the repositories above, and update the commit and date at the
top of this file. A picture that no longer matches the documents is a
picture to replace, not to keep.
