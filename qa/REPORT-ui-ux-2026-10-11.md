# UI/UX audit, 2026-10-11

An audit of TRELLIS's three interfaces for usability friction, information
flow (how a person finds, filters and acts on what trellis reports) and
control (settings, navigation, feedback, state visibility), measured against
the guidance in nextlevelbuilder/ui-ux-pro-max-skill (commit 67803ea; its
`data/ux-guidelines.csv`, `data/app-interface.csv`, `references/pro-rules.md`
and `references/quick-reference.md`). Every finding is a GitHub issue
labelled `ui`, `ux`, `audit` and `priority:high|medium|low`, with the
standard dimension labels computed by `trellis-tickets new` (docs/TICKETING.md).

## Method

- Code read on `dev` at 5b308f2 (findings cite `file:line` there):
  `desktop/ui/*.js`, `desktop/ui/style.css`, `desktop/ui/index.html`,
  `desktop/src/state.rs`, `desktop/src/lockdown.rs`; `crates/tui-demo/src/tui/*.rs`,
  `crates/trellis-cli/src/tui_run.rs`, `tui_bridge.rs`; the CLI's
  `start_screen.rs`, `first_run.rs`, `overview.rs`, `output.rs`, `doctor_cmd.rs`,
  `log_cmd.rs` and `main.rs`'s help text. Contrast ratios were computed from the
  `style.css` tokens (WCAG relative luminance), not estimated.
- Captures, in `qa/screenshots/ui-ux-2026-10-11/`: the desktop page served by
  `trellis-desktop --web` (#375, branch t375-web-view) in headless Chromium at
  1280×820, light and dark, with `TRELLIS_CONFIG_DIR` empty and no runtime on
  the machine, against the `qa/fixtures/trellis-demo.bundle` repository and a
  `scripted:` model (`qa/fixtures/scripted-done-after-edit.json`); `tui_demo`
  in an 80×24 pseudo-terminal rendered with pyte (`tui/*.txt`, each with its
  size and keys on line 1); five CLI outputs (`cli/*.txt`). Local paths in the
  CLI captures are written as `<scratch>` and `<checkout>`.
- What the captures could not show is marked "unverified" in the issue.

## Rubric

Sources are files in nextlevelbuilder/ui-ux-pro-max-skill at 67803ea. Touch, safe-area and gesture rules are left out; `pro-rules.md` scopes them to mobile.

- R1 Text contrast is at least 4.5:1 (3:1 for large text), measured separately in light and dark. `data/ux-guidelines.csv` #36; `references/pro-rules.md` (Light/Dark Mode Contrast)
- R2 Control boundaries, focus indicators and meaningful glyphs are at least 3:1 against what is next to them, and borders show in both themes. `references/pro-rules.md` (Icon Contrast; Border and divider visibility); `ux-guidelines.csv` #102
- R3 Colours are semantic tokens per theme, and light and dark are designed together. `references/quick-reference.md` §4 dark-mode-pairing, §6 color-semantic, color-dark-mode
- R4 Colour is never the only carrier of meaning (pass/fail, wrote, integrity); add a word or a glyph. `ux-guidelines.csv` #37; `quick-reference.md` §1 color-not-only
- R5 Dense data uses 12–14 px type and tabular figures, with hierarchy by size, weight and spacing. `data/styles.csv` No 28 Data-Dense Dashboard; `quick-reference.md` §5 visual-hierarchy, §6 number-tabular
- R6 Headings run in order with one h1, and semantic landmarks are used. `ux-guidelines.csv` #39, #42
- R7 Navigation shows the current location and stays in one place. `ux-guidelines.csv` #3; `quick-reference.md` §9 nav-state-active, navigation-consistency
- R8 On a view change, focus moves to the new content; a skip link passes repeated blocks. `quick-reference.md` §9 focus-on-route-change; `ux-guidelines.csv` #45
- R9 Scroll, filter and input are kept; never yank the reader's position. `quick-reference.md` §9 state-preservation; `data/app-interface.csv` #12
- R10 An unavailable action says why and what to do; no dead ends. `quick-reference.md` §9 empty-nav-state; `ux-guidelines.csv` #80
- R11 Empty states say what is missing and the next action. `ux-guidelines.csv` #79; `quick-reference.md` §10 empty-data-state
- R12 Loading feedback matches the wait; long waits are named; no blank or frozen screen. `ux-guidelines.csv` #10, #78; `quick-reference.md` §7 loading-states
- R13 A control is disabled while its request is out. `ux-guidelines.csv` #32
- R14 Errors are specific, placed next to their cause, give a recovery path, and are announced. `ux-guidelines.csv` #33, #44, #55; `quick-reference.md` §8 error-recovery
- R15 A completed submit is acknowledged and says the next step. `ux-guidelines.csv` #34, #61, #83
- R16 Destructive actions are confirmed, use the danger colour, and sit apart from routine actions. `ux-guidelines.csv` #35; `quick-reference.md` §8 destructive-emphasis, §9 destructive-nav-separation
- R17 Modals state their keys and escape route, and their content is fully reachable. `quick-reference.md` §1 escape-routes, §9 modal-escape; `app-interface.csv` #11; `ux-guidelines.csv` #100
- R18 Everything works from the keyboard, tab order follows visual order, a composite widget is one tab stop, and drag has a key path. `ux-guidelines.csv` #41, #103; `quick-reference.md` §1 keyboard-nav
- R19 Every control has a visible focus ring of 2 px at 3:1. `ux-guidelines.csv` #28, #102
- R20 Controls have accessible names and use native elements; hover-only text (`title` tooltips) is also reachable by keyboard. `ux-guidelines.csv` #40, #116; `quick-reference.md` §10 tooltip-keyboard
- R21 Live changes are announced once as a contextual phrase; logs append rather than re-announce. `ux-guidelines.csv` #118; `app-interface.csv` #4
- R22 Reduced motion is honoured, with no decorative or infinite motion. `ux-guidelines.csv` #9, #12
- R23 A graph has a list or table alternative as its source of truth, with keyboard details. `data/charts.csv` No 16 (network graph: adjacency list, Enter drills); `quick-reference.md` §10 data-table, focusable-elements
- R24 Long lists filter and sort on the fields people scan (time, status), with visible filter state and a way back. `styles.csv` No 28 checklist; `ux-guidelines.csv` #90
- R25 Status is visible from every view (running, waiting, failed), and counters move while work runs. `data/ui-reasoning.csv` No 81 Developer Tool (real-time monitor); `charts.csv` No 23
- R26 Keyboard shortcuts are documented where they act, and system keys (paste, Ctrl-C, Esc) behave as users expect. `ui-reasoning.csv` No 81 must_have keyboard-shortcuts; `quick-reference.md` §1 keyboard-shortcuts
- R27 Inputs have a visible label and helper text for anything non-obvious, such as Enter sending. `ux-guidelines.csv` #54, #62; `quick-reference.md` §8 input-helper-text
- R28 One primary action per view, with secondary options disclosed progressively. `quick-reference.md` §4 primary-action, §8 progressive-disclosure
- R29 Long operations show progress; stream rather than block. `ux-guidelines.csv` #81, #93
- R30 Essential and distinguishing text (state, names, which runtime) is shown whole, never clipped or replaced by an opaque code. `ux-guidelines.csv` #113, #116

## Findings

| Priority | Finding | Issue | Surface | Title |
|---|---|---|---|---|
| high | UX-01 | #634 | tui | Make Ctrl-C and Esc during a TUI run stop and roll back the run, or stop claiming they do |
| high | UX-02 | #635 | tui | Stop the TUI's approval answer from relabelling edits that were already written |
| high | UX-03 | #636 | tui | Make the TUI's worktree state true after each run and keep it on screen at 80 columns |
| high | UX-04 | #637 | desktop | Separate Stop from Send in the desktop composer, mark it as destructive and confirm it |
| high | UX-05 | #638 | desktop | Tell the desktop user trellis rpc is not running, instead of asking for a model, and offer to start it again |
| high | UX-06 | #639 | tui | Paste multi-line text into the TUI as one line instead of submitting each line |
| high | UX-07 | #640 | tui | Let the TUI approval modal scroll and keep the question in view |
| high | UX-08 | #641 | tui | Show unreported TUI budget figures as unreported and keep elapsed live between turns |
| high | UX-29 | #642 | cli → desktop | Fix trellis's stderr colour codes on a pipe: the desktop's trellis messages pane shows raw SGR sequences |
| medium | UX-09 | #643 | desktop | Keep the desktop actions log and messages pane where the reader scrolled them |
| medium | UX-10 | #644 | desktop | Say what Enter does in the desktop composer, acknowledge Send, and let starters be reviewed before they run |
| medium | UX-11 | #645 | desktop | Move "Open another repository" out of the model pane, name the open repository in the window title, and get the overview above the fold |
| medium | UX-12 | #646 | desktop | Move keyboard focus to the shown section's heading when a rail button is pressed, and give the page one h1 |
| medium | UX-13 | #647 | desktop | Give desktop form fields a 3:1 border and the page's focus ring in both themes |
| medium | UX-14 | #648 | desktop | Give the codebase graph a module list keyboard and screen-reader users can use, and take circles out of the tab order |
| medium | UX-15 | #649 | desktop | Show each trail's start time and integrity in the trails list, and let the filter match outcome and integrity |
| medium | UX-16 | #650 | cli | Print what the CLI is waiting on before each long bounded wait: doctor probes, the first-run probe, bare trellis's overview |
| medium | UX-17 | #651 | cli | Spell out recorded checks and the radius's reach and doubt in trellis log instead of +/- suffixes and one total |
| medium | UX-26 | #652 | desktop | Stop the desktop status bar from wrapping: show the trellis binary and root as names with the full path on hover and in the title |
| medium | UX-27 | #653 | desktop | Use one vocabulary for a run's outcome across the Run, Trails and Overview panes |
| medium | UX-28 | #654 | cli + desktop | Say in the overview's codebase pane that the desktop indexed the repository in memory, instead of "not indexed; run trellis index" |
| medium | UX-30 | #655 | tui | Stop the TUI's right column and budget line truncating at 80 columns, and wrap /help at word boundaries |
| low | UX-18 | #656 | desktop | Let the board filter by status, and use one status word for a task in the overview and the board |
| low | UX-19 | #657 | desktop | Disable Show radius and vault Search while their answer is out |
| low | UX-20 | #658 | desktop | State the approval dialog's keys and make its diff keyboard-scrollable |
| low | UX-21 | #659 | desktop | Show a reading state in each overview pane and put the Open refusal next to its button |
| low | UX-22 | #660 | tui | Add PgUp, PgDn, Home and End to the TUI transcript |
| low | UX-23 | #661 | tui | Say "wrote" in text on the TUI action node, not only in yellow |
| low | UX-24 | #662 | cli | Make trellis --help match what bare trellis and --web do today |
| low | UX-25 | #663 | cli + desktop | Name the runtime beside each model in the first-run picker, and say in words why a runtime was not found |

Each issue carries: problem, user impact, current vs expected, screenshots,
affected components, suggested fix (held to desktop/CLAUDE.md: `textContent`
only, no inline style, no new CSP source, radius never a ratio, unreported
never 0), acceptance criteria as `## Done when` with a fenced `## Verify`,
dependencies and an effort estimate.

Findings UX-26 to UX-30 came from the captures rather than the code read:
the wrapped status bar, the three vocabularies for one run, the overview's
"not indexed" beside an indexed codebase, tracing's colour codes in the
messages pane, and the 80-column truncation.

## Keep these

1. Strings reach the DOM only through `textContent` and `createElement(NS)`
   (held by `desktop-guards.sh` and `page_has_no_style_attribute`); trail text
   arrives as `{untrusted}` and renders in its own face (`trails.js:27-38`).
2. Unreported figures say so and are never 0 (`render.js:124-198`,
   `overview.js:38-40`, `radius.js:35-37`); new panes should reuse those helpers.
3. Radius is three counts with "a floor on reach" (`render.js:100-118`); doubt
   is titled "a different population" (`radius.js:94-117`); Verified and
   Reviewed stay two rows.
4. All text passes contrast in both themes (lowest 4.69:1 light, 5.79:1 dark);
   `style.css` has no transition or animation, so reduced motion holds by
   construction.
5. Approvals fail closed: the dialog focuses Reject and treats Escape as no;
   the core forwards an approve only for an open question
   (`src/commands.rs:553-568`); the TUI modal swallows other keys.
6. Views keep their state across rail switches; stale answers are dropped by
   request id (`trails.js`, `radius.js`, `queue.js`).
7. The TUI follows the transcript end unless scrolled back, shows a too-small
   screen instead of breaking the layout, and lets no control character reach
   the terminal (`tui/view.rs:1012-1043`).
8. The CLI start screen confirms `git init` with [y/N] and refuses it in the
   home directory; all trail and `gh` text goes through `plain()`.

## Captures

Desktop (`desk-*.jpg`): 01/02 landing light and dark; 03 composer disabled;
04 model pane with the repository form; 05 empty run section; 06 focus after a
rail click; 07/08 field borders (trails, codebase) light and dark; 09 codebase
indexed; 09b model chosen; 09c run in flight; 10 after a scripted run (10b full
page); 11 trails after the run (11b a record); 12 overview after the run;
13 rpc exited (13b the messages pane); 14 no repository; 15 approval dialog.
TUI (`tui/*.txt`): launch idle, mid-run, approval modal, after reject, /help,
graph, model, scrolled up, second run, long approval, after approve.
CLI (`cli/*.txt`): `--help`, `tui --help`, `log`, `doctor` against a closed
port, `overview` in and outside a repository.
