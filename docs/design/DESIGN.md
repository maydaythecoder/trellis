# TRELLIS design

What the interface is for, the rules it keeps, and the system (tokens, type,
space, components) the two clients draw from. Written down 2026-10-25 so that
the direction stops living in the owner's head and in one wireframe.

Status of each part is marked. "Decided" is the owner's call; "held" means a
test or check enforces it; "open" means nobody has decided and the clients
should not guess.

## 1. What the interface is

**TRELLIS is a bare harness.** Agents, subagents and context management are
the defaults; the call graph is one tool among them. The interface shows a
harness running, not a product selling itself.

**The user is the conductor.** (Decided.) The agents do the work; the person
directs it. Every control on screen is a conductor's control: start, steer,
queue, stop, approve. The interface never asks the person to do the agents'
job for them and never does the person's job of judging.

**Change is the main visual event.** (Decided.) What draws the eye is what
just changed: a new row in the log, a file that moved from `in the worktree`
to `awaiting your approval`, a state word that flipped. Static chrome is
quiet; the run is loud. There are no decorations that move when nothing has
happened.

**One overview that burrows inward.** (Decided.) No tabs. The person starts
at the run's facts (state, elapsed, turn, budget), moves into the log, then
into one file, then into one diff, and each level is reached by going deeper
in the same frame, not by switching to another screen. The wireframe's levels
`run`, `code`, `summary` are the three depths; `tui` is the same three at
terminal width.

**Lines, connectors and glyphs.** (Decided.) The graphics are the ones a
terminal can draw: rules, box frames, connectors between a tool call and
its result, ASCII and Unicode glyphs for state. The desktop uses the same
vocabulary at higher resolution, not a different one.

**Clean, and as playful as the person wants.** (Decided.) Clean by default:
one type family for what the engine said, one for what a person wrote,
square panels, few colours each with a meaning. Play is allowed in the
places the person owns (names, the compose box) and in the wire-only
annotation colour, and never in a figure, a state word or a check row.

**Two faces of one system.** (Decided.) The desktop (`desktop/`, Tauri,
ADR 0048) and the TUI (`tui/`, Go and Bubble Tea, ADR 0052) are two clients
of one engine, `trellis rpc`. They share the state words, the verbs, the
region order and the meaning of each colour. They differ in resolution and
input, not in vocabulary. Parity is tracked row by row in `tui/PARITY.md`.

**No single reference image.** (Decided.) The run view is assembled from the
evidence in `run-view-research-2026-10-25.md`, which surveys sixteen
harnesses and the oversight literature. Where one product had a good
mechanic it is borrowed and named; nothing is copied whole.

## 2. Rules the interface keeps

These are not taste. Each comes from an ADR, CLAUDE.md or a named test, and a
change that breaks one fails review. The full list with its sources is in
`.claude/skills/trellis-ui/SKILL.md`; the short form:

1. The client computes no verdict. A check row exists only because
   `trellis-checks` produced a `Verdict`. (Held: ADR 0004;
   `TestPanesCheckRowsComeOnlyFromASomeVerdict`.)
2. A figure trellis did not report is `unreported`, never `0`. (Held:
   `TestBudgetLineUnreportedFiguresAreNamedNotDrawnAsZero`,
   `session_tokens_unknown_is_null_never_zero`.)
3. `Verified` and `Reviewed` are two rows, never a sum. Blast radius is three
   counts, never a ratio. (Held: ADR 0003, ADR 0014.)
4. No currency, no percent-complete, no remaining-time estimate. Elapsed
   counts up. (Decided: brief §4.11; the research note's CHI 2026 finding.)
5. One outcome vocabulary: `converged`, `not_converged`, `interrupted`,
   `error`, with the exit code. (Held: #653.)
6. Everything from the stream goes into the view as text, never markup.
   Trail text is fenced and marked untrusted. (Held: ADR 0048 §5, rule 10.)
7. Only contract fields. A new region needs `client_contract.json` and
   ADR 0048 §6 amended first. (Held: ADR 0048 §6.)
8. No marketing language. "Done" is `run_end`; nothing is "complete".
9. A steer is text for the next model call. It never approves an edit or a
   check. (Held: ADR 0004; `TestSteerDeliveredOnceAtItsTurn`.)

## 3. The system

The source of truth for every value below is the first `<style>` block of
`run-view-wireframe.html`. This section is a copy for reading;
`scripts/checks/design-tokens.py` fails when the two drift, and
`scripts/checks/design-tokens.py --write` regenerates the tables from the
wireframe.

### 3.1 Colour

Each colour has one meaning. A region picks the colour for what the thing
*is*, never for emphasis.

| Token | Means |
|---|---|
| `--signal` (+ `-hover`, `-soft`, `-ring`) | The person: their prompt, their steer, and what needs them (`needs you`, approval). |
| `--cool` (+ `-soft`, `-fill`) | The engine's own activity: tool calls, the runtime, edges that are being followed. |
| `--ok` (+ `-soft`) | A check that passed, as recorded by `trellis-checks`. Nothing else. |
| `--down` (+ `-soft`) | A check that failed, a refusal, stop. |
| `--warn` | Waiting, a WARN line. |
| `--play` (+ `-soft`) | Wire-only annotations (the `sources` tags). Never in a shipped client. |
| `--up` | Scroll and direction hints. |
| `--diff-add`, `--diff-del`, `--diff-hunk` (+ `-soft`) | Diff lines and hunk headers. |
| `--series-1` … `--series-5` | Ordered series in any small chart; `series-1` is the person's. |
| `--ground`, `--panel`, `--panel-raised`, `--panel-inset` | Surfaces, from the page down into a well. |
| `--line`, `--line-strong`; `--edge`, `--edge-active`, `--edge-cool`; `--grid-dot` | Rules, connectors, and the dot grid behind a graph. |
| `--ink`, `--ink-secondary`, `--ink-muted`, `--ink-inverse` | Text, in four weights of attention. |
| `--frame`, `--frame-strong`, `--frame-signal`, `--frame-down` | One-pixel box frames as `box-shadow`, so a frame never changes layout. |
| `--scrim` | Behind a modal. |

Values, both themes:

<!-- tokens:colour -->
| Token | Dark | Light |
|---|---|---|
| `--ground` | `#0b0b0d` | `#f2f1ec` |
| `--panel` | `#121216` | `#ffffff` |
| `--panel-raised` | `#18181d` | `#f7f6f2` |
| `--panel-inset` | `#0e0e11` | `#e9e8e2` |
| `--line` | `#26262c` | `#d9d8d1` |
| `--line-strong` | `#3a3a42` | `#b9b8b0` |
| `--ink` | `#ececf1` | `#141418` |
| `--ink-secondary` | `#a8a8b3` | `#4a4a55` |
| `--ink-muted` | `#80808c` | `#70707b` |
| `--ink-inverse` | `#0b0b0d` | `#ffffff` |
| `--signal` | `#ffb224` | `#9a5b00` |
| `--signal-hover` | `#ffc553` | `#7e4a00` |
| `--signal-soft` | `#2a1f07` | `#fbead0` |
| `--signal-ring` | `#ffb224` | `#9a5b00` |
| `--cool` | `#7cc4ff` | `#2a5bd7` |
| `--cool-soft` | `#0f2236` | `#e3ecfb` |
| `--cool-fill` | `#4a5f91` | `#4a5f91` |
| `--play` | `#c8f04a` | `#527a00` |
| `--play-soft` | `#1f2a07` | `#eef6d6` |
| `--up` | `#7cc4ff` | `#2a5bd7` |
| `--down` | `#ff6a3d` | `#b83d12` |
| `--down-soft` | `#2e130b` | `#fbe5dc` |
| `--warn` | `#ffb224` | `#9a5b00` |
| `--ok` | `#34d399` | `#0f7a52` |
| `--ok-soft` | `#0b2a1f` | `#dff4ea` |
| `--diff-add` | `#34d399` | `#0f7a52` |
| `--diff-add-soft` | `#0b2a1f` | `#dff4ea` |
| `--diff-del` | `#ff6a3d` | `#b83d12` |
| `--diff-del-soft` | `#2e130b` | `#fbe5dc` |
| `--diff-hunk` | `#7cc4ff` | `#2a5bd7` |
| `--series-1` | `#ffb224` | `#9a5b00` |
| `--series-2` | `#7cc4ff` | `#2a5bd7` |
| `--series-3` | `#ececf1` | `#141418` |
| `--series-4` | `#a8a8b3` | `#8a8a94` |
| `--series-5` | `#ff6a3d` | `#b83d12` |
| `--grid-dot` | `#3a3a42` | `#c6c5bd` |
| `--edge` | `#3a3a42` | `#b9b8b0` |
| `--edge-active` | `#ffb224` | `#9a5b00` |
| `--edge-cool` | `#7cc4ff` | `#2a5bd7` |
| `--scrim` | `#000000b3` | `#14141866` |
| `--frame` | `0 0 0 1px #26262c` | `0 0 0 1px #d9d8d1` |
| `--frame-strong` | `0 0 0 1px #3a3a42` | `0 0 0 1px #b9b8b0` |
| `--frame-signal` | `0 0 0 1px #ffb224` | `0 0 0 1px #9a5b00` |
| `--frame-down` | `0 0 0 1px #ff6a3d` | `0 0 0 1px #b83d12` |
<!-- /tokens:colour -->

Contrast of every ink on every surface in both themes: not yet measured
(open). The check that measures it is a follow-up to #786.

### 3.2 Type, space, radius

`--font-mono` for anything the engine said or measured; `--font-sans` for
prose a person or the model wrote. Figures use tabular numerals. Panels are
square (`--radius-0`); only dots are round (`--radius-full`); graph nodes
get `--radius-node`.

<!-- tokens:base -->
| Token | Value |
|---|---|
| `--space-0.5` | `2px` |
| `--space-1` | `4px` |
| `--space-1.5` | `6px` |
| `--space-2` | `8px` |
| `--space-3` | `12px` |
| `--space-4` | `16px` |
| `--space-5` | `20px` |
| `--space-6` | `24px` |
| `--space-8` | `32px` |
| `--space-12` | `48px` |
| `--space-16` | `64px` |
| `--radius-0` | `0px` |
| `--radius-xs` | `2px` |
| `--radius-node` | `6px` |
| `--radius-sm` | `4px` |
| `--radius-md` | `8px` |
| `--radius-full` | `9999px` |
| `--font-mono` | `"JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace` |
| `--font-sans` | `"IBM Plex Sans", ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif` |
<!-- /tokens:base -->

### 3.3 Regions and components

The run view's regions, in reading order, with what each is made of. The
full description with every state is in `.claude/skills/trellis-ui/SKILL.md`
and the wireframe itself.

| Region | Components | Draws from |
|---|---|---|
| Now strip | state word, elapsed, `-ing` activity line, `turn n / max`, queued count, `Stop` / `Pause` / `Summary so far` | `run_start`, last event, `run_end` |
| Action log | role-typed rows (you · task, you · steer, model said, tool call, check, note, refusal, model asks you), turn dividers, filters, quiet mode | the event stream |
| Compose | one box, three verbs on the button: `Steer`, `Queue`, `Stop and send`; the queue with per-item state | steer channel, `follow_up` |
| Where it is | phase, tool and target, turn, last call, prompt bytes vs context budget, compactions, model, tier, runtime, control, root | `turn`, `run_budget`, `get_state` |
| Budget | two numbers per row (used / ceiling); wall clock excludes waiting on a person | `run_budget` |
| Code | files touched with two line counts and a state; `diff` and `whole file`; approve/reject only while waiting on that edit; a line comment becomes a steer | `approval_request`, the worktree |
| Summary | outcome and exit code, checks by kind with duration, facts, budget consumed, steers, path by turn, files, trail id and integrity, closing words fenced | `run_end`, the trail |

### 3.4 The TUI face

The TUI shows the same regions at 120×40 and folds the right column under
the transcript at 80 columns. Same state words, same verbs, keyboard only.

How the colour tokens map to terminal colours (Lip Gloss `AdaptiveColor`,
dark and light) is open: `tui/internal/view/view.go` carries its own
styles today and no table ties them to the names above. That mapping is the
next design ticket after #786, and the parity row for it goes in
`tui/PARITY.md`.

### 3.5 The desktop face

`desktop/ui/style.css` still carries the older token names (`--bg`, `--fg`,
`--accent`, `--passed`, `--failed`, `--reach`, `--doubt`, …). Mapping them
onto the names above is part of building the wireframe in the desktop, not
a side change, and is tracked under #786's follow-ups.

## 4. Adding to the system

1. A new colour needs a meaning that no existing token has. If the meaning
   exists, use its token.
2. A new region needs its event or command named first (rule 7), then the
   wireframe with its `sources` tag, then the owner's read, then the two
   clients, then its parity row.
3. Change the wireframe's `<style>` block, run
   `scripts/checks/design-tokens.py --write`, and commit both.
