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

**The brand book lives in the repository.** (Decided, 2026-10-25.)
`system/` holds the owner's design system as he wrote it: `README.md` (the
brand book: principles, content, colour, type, depth, motion, vocabulary),
`tokens.json` (every value), `faces.md` (how the TUI and the desktop draw the
same model), `events.md` (the event-to-mark intent), one card per component
in `components/`, the icons in `icons/` and the mark in `logos/`. This file
adds the engine's rules to the brand book and wins where the two differ;
§2.1 lists each difference so a client follows a rule rather than a guess.

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

### 2.1 Where the brand book and the engine's rules differ

The brand book describes the product the owner wants; the rules above hold
what the engine reports today. Where they differ, a client does what the last
column says. A row marked `not built` becomes a ticket for the engine (and an
ADR where the contract changes), never a client-side approximation. The
commands `trellis rpc` accepts on 2026-10-25 are `prompt`, `steer`,
`follow_up`, `abort`, `approve`, `compact`, `set_model`, `quit` and the
read-only `get_*`, `list_*`, `classify` and `vault_*` queries
(`crates/trellis-cli/src/rpc.rs`).

| The brand book says | The rule here | What a client does |
|---|---|---|
| Spend is a Stat in the user's currency, never hidden ("Tokens brokered"; the Budget card's cost row). | No figure trellis did not report; under the Local regime spend is not metered (rule 4). | A cost row appears only when the engine reports a cost; otherwise it reads `not metered`. |
| Agent cards, subagents, `agent.spawn` with a lime beat (Agent card, `events.md`). | One loop and one model in v1; multi-agent work is deferred, and no agent judges another's output (CLAUDE.md rule 5). | No Agent card. "Where it is" shows the loop's phase, tool, target and model. |
| Context is a Meter with a percentage readout, amber past 70%, orange past 90% (Agent and Meter cards). | Two numbers, never a percent-complete; prompt size is in bytes where bytes were measured (rules 2 and 4). | The Meter's cells may fill to the share and keep the 70/90 tones; the readout is `used / ceiling` in the unit measured, with no percent sign. |
| Budget limits extend with `+step` or an input, mid-run (Budget card). | No command changes a budget during a run; a change would be the user's action in the trail (ADR 0008: budgets divide, never multiply). | The control is drawn and marked `not built` until the ADR that adds it is accepted. |
| Plan steps and the reasoning are editable and sent as steers (Plan card). | The `plan` event (ADR 0058 §5, #810) carries the model's steps when a repository enables the `plan` tool; they are the model's words (rule 1), and no command edits them. No event carries the reasoning. | The list is drawn from the latest `plan` event; a person's edit to it, or to the model's last message, is sent as a `steer`. With no plan event, `Reply as a steer` under the model's last message. |
| Queue items are reordered, edited and cancelled with ↑ ↓ ✎ × (Queue card). | Steers are delivered in order before the next model call; no command cancels or edits a queued item. | Each item shows `queued`, `delivered after turn N` or `not delivered`. Reorder applies only among follow-ups the client still holds; cancel and edit are `not built` for anything already sent. |
| Run states `running · waiting on you · paused · failed · done · idle` (NowStrip card). | `run_end`'s words: `converged`, `not_converged`, `interrupted`, `error`, with the exit code (rule 5). | Live states as the card names them (`paused` only once pause exists); an ended run shows its outcome word and exit code, never `done` or `failed`. |
| A finished run gets `\(^o^)/  all 212 passed` (README, Ascii card). | Counts come only from `trellis-checks` (rule 1). | A face may mark the outcome; any count beside it is a recorded check's count, never the model's. |
| Components ship as React on `window.Trellis` with `components/bundle.css` (README, "Components"). | The desktop is plain ES modules with no bundler and a CSP without `'unsafe-inline'`; the TUI is Go (`desktop/CLAUDE.md`, ADR 0052). | The cards and `components/bundle.css` are references. Each client builds the component in its own code. |
| Meter widths and Sparkline paths are drawn per value (Meter, Sparkline cards). | No `style` attribute and no `.style` from script on the desktop (`desktop/CLAUDE.md`). | Meters are block characters in text (`█░`), the same in both faces; anything else that varies takes a class, never an inline style. |
| Event names `session.start`, `tool.call`, `context.update`, … (`events.md`). | A client reads only the contract's names (`run_start`, `turn`, `run budget`, `steer delivered`, …) and fields (rule 7). | `system/events.md` is intent. The mapping from the contract's names to each mark is not written yet (open); when it is, it is its own document, `docs/design/events.md`. |
| JetBrains Mono for numerals, labels, keycaps and code, IBM Plex Sans for headings and prose, both from Google Fonts (README, "Type"). | The desktop loads nothing from a network, and its CSP (`default-src 'none'`, no `font-src`) changes only by amending ADR 0048 (`desktop/CLAUDE.md`); the TUI draws in the terminal's font. | Fonts come from the machine. The stacks in `tokens.json` (`type.families`) name the two faces first and fall back to the system's (`ui-monospace`, `system-ui`, …), so each face shows where it is installed. Nothing is bundled, and the CSP keeps its directives. |
| Focus is a 1px `signal-ring` outline, offset 1px; on a panel the whole frame turns `signal` (README, "States and motion"). | The desktop draws one 2px `--focus` outline on every button, field and summary, so WebView2 and WebKitGTK show the same ring (`field_contrast_focus_ring_covers_every_field_and_summary` in `desktop/src/lockdown.rs`). | The desktop's focus ring stays 2px wide, a deliberate difference from the brand book: WCAG 2.2's focus-appearance criterion (2.4.13) asks for at least the area of a 2px perimeter, which a 1px ring does not give. |
| The composer's third verb, `Stop and send` (§3.3, from the run-view wireframe): stop this run, then run the message. | `trellis rpc` refuses a `prompt` while a run is in flight ("one at a time"), answers `abort` only once the run has ended and rolled back, and an abort also drops the follow-ups queued behind the run (`crates/trellis-cli/src/rpc.rs`). | The client sends `abort`, holds the message, and sends it as a `prompt` once that run's `run_end` arrives. Where the client keeps its state sequences it (the desktop's core, the TUI's model), on the event and never on a timer. |

The fonts, focus-ring and stop-and-send rows were taken on 2026-10-25 by the
session under the owner's instruction to work unattended (#795); each is
reversible.

## 3. The system

The source of truth for every value below is `system/tokens.json`, the owner's
design system as vendored on 2026-10-25. Four copies are checked against it
by `scripts/checks/design-tokens.py`: the tables in this section, the TUI's
theme table (`tui/internal/theme/tokens_gen.go`, §3.4) and the desktop's
token block (`desktop/ui/style.css`, §3.5), which
`scripts/checks/design-tokens.py --write` regenerates, and the token block at
the top of `run-view-wireframe.html`, which a person edits. To change a
value: edit `tokens.json`, edit the wireframe's block to match, run
`--write`, commit all five.

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
| `--play` (+ `-soft`) | The playful register: an Ascii face, a loader, the mark on a finished run; in the wireframe, also its wire-only `sources` tags. Never a status, an action or a figure, and absent when the register dial is set to serious. |
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

The brand book gives each colour role a terminal colour: a 256-colour index
for most terminals and, in each row's text, the 16-colour fallback for a
terminal that reports no more. A truecolor terminal uses the hex values in
§3.1. Frames are box-drawing characters, meters `█` and `░`, sparklines
`▁▂▃▄▅▆▇█`; `system/faces.md` has the full list of what the terminal draws
with less.

The theme table that maps these rows into Lip Gloss,
`tui/internal/theme/tokens_gen.go`, is generated by
`scripts/checks/design-tokens.py --write`, and the check fails when it is
stale; its parity row is in `tui/PARITY.md`. It gives every value explicitly
as a `CompleteAdaptiveColor`, because Lip Gloss's own downgrading picks different
colours from this table (at 16 colours `line` becomes black and `play` the
same yellow as `signal`). A row's 16-colour value is read from its text: the
index beside its colour's name (the one marked for dark terminals where a
row gives two), no colour for `default background` or `reverse video`, and
the named row's for `Same as`; a row the check cannot read fails it.
`tui/internal/theme/theme.go` builds every style once from one renderer; a
raised row with no fill at the terminal's depth is drawn in reverse video.
`trellis-tui` reads the terminal's colour depth and background once, before
its program starts, and `--theme dark|light` replaces the background the
terminal answers (Windows always answers dark). The view paints each row by
its role, after every cut and pad is made on the plain text
(`tui/internal/view/paint.go`, #841): what the person typed in `signal`,
the model's words in `ink`, tool calls in `cool`, a check in `ok` or `down`
from its verdict only, notes in `warn`, titles and captions in
`ink-secondary`, and frames in `line`. Each transcript row carries the role
its writer gave it (`tui/internal/model/rows.go`, #849): what a tool
returned is `ink-secondary` under its call and an error or a refusal is
`down`; no role is read from a row's words.

The design system's components are drawn in one place,
`tui/internal/view/components.go` (#911), and every region draws them from
there. A Meter is a fixed number of cells, `█` filled and `░` empty, then
its two figures (`████░░░░ 3/40`): its cells are `cool`, `signal` past 70%
of the ceiling and `down` past 90% (§2.1), its empty cells `line-strong`,
and it never says a percentage. A Status is a dot and its word in one tone
(`● 1 pending`), and a keycap is the key in brackets (`[TAB] select`). A
figure trellis did not report is drawn `—`, which `/help` names as
unreported (rule 2), and the clock this view keeps for itself is marked
`*` until trellis reports its own (#906).

The table gives 256- and 16-colour values for dark terminals. A light
terminal (owner decision, 2026-10-25) takes the light hex values of §3.1 at
truecolor; below truecolor, `ink`, `ink-secondary` and `ink-muted` take the
terminal's own foreground and every other role its 16-colour value, at both
depths. The soft fills and `ink-inverse` have no terminal value: below
truecolor a soft fill is not painted and `ink-inverse` is reverse video.
Neither is in the theme yet; the change that first paints one adds it
through `design-tokens.py`.

<!-- tokens:terminal -->
| Token | 256-colour index | Usage, with the 16-colour fallback |
|---|---|---|
| `term-ground` | `232` | Page. 16-colour: default background (never paint it). |
| `term-panel` | `233` | Panel fill. 16-colour: default background; the frame alone carries the panel. |
| `term-panel-raised` | `235` | Hover and selected row fill. 16-colour: reverse video on the row. |
| `term-line` | `237` | Box-drawing frames and rules (│ ─ ┌ ┐ └ ┘ ├ ┤). 16-colour: bright black (8). |
| `term-line-strong` | `240` | Focused frame when signal is unavailable, grid dots (·). 16-colour: bright black (8). |
| `term-ink` | `255` | Primary text and numerals. 16-colour: bright white (15). |
| `term-ink-secondary` | `248` | Labels and body. 16-colour: white (7). |
| `term-ink-muted` | `244` | Metadata. 16-colour: bright black (8) — the one place contrast drops below 4.5:1; keep it to timestamps. |
| `term-signal` | `214` | The hot accent: active frame, selected text, the followed edge. 16-colour: yellow (3). |
| `term-cool` | `111` | Links, second series, up. 16-colour: blue (4), bright blue (12) on dark terminals. |
| `term-play` | `191` | Faces, loaders, celebrations. 16-colour: bright green (10). |
| `term-ok` | `78` | Healthy status. 16-colour: green (2). |
| `term-down` | `202` | Failed, removed lines, negative deltas. 16-colour: red (1). |
| `term-warn` | `214` | Same as signal; always with a word. |
| `term-edge` | `240` | Connector characters (─ │ ┐ └ ╭ ╮) at rest. 16-colour: bright black (8). |
| `term-edge-active` | `214` | The followed connector. 16-colour: yellow (3). |
<!-- /tokens:terminal -->

### 3.5 The desktop face

`desktop/ui/style.css` takes its colours from the tokens above (#813). Its
block between `/* tokens:start` and `/* tokens:end */` is generated by
`scripts/checks/design-tokens.py --write`: Dark is the default, and
Daylight is written under `[data-theme=light]` and the light
`prefers-color-scheme` query, since the page has no toggle and the system's
setting decides. Rules use the token names (`ground`, `panel`, `ink`,
`signal`, `ok`, `down`, `diff-add`, …), and no colour is written anywhere
else but the dialog's `::backdrop`, whose `var(--scrim, …)` fallback says
why. Below the block sit the desktop's own tokens, which the brand book has
no name for: `--control-border` (a field's edge, measured at 3:1 or more on
`panel` and `ground`) and the Atlas's map ramps (regions, cells, callers and
callees, `reach`, `doubt`, `churn-1` to `churn-5`), each said why in a
comment there.

### 3.6 Icons and the mark

`system/icons/` holds 31 icons for TRELLIS's nouns (agent, task, diff,
context, model-local, model-escalated, queue, run, pause, stop, budget,
commit, trail, call-graph, blast-radius, burrow-in, burrow-out, …): 16×16,
1.5px stroke, `currentColor`, so an inlined icon takes the colour of the
text beside it. `system/logos/` holds the mark (a 3×3 lattice with one lit
crossing), the wordmark, the favicon and the app icons. The desktop uses
these; the TUI uses the glyphs the brand book lists instead (`● ○ ◆ ▲ ⤢ ⏎ ⟩`).
An icon is never the only carrier of a meaning: a status is a word with a
dot, and an icon sits beside its label.

## 4. Adding to the system

1. A new colour needs a meaning that no existing token has. If the meaning
   exists, use its token.
2. A new region needs its event or command named first (rule 7), then the
   wireframe with its `sources` tag, then the owner's read, then the two
   clients, then its parity row.
3. Change `system/tokens.json` and the wireframe's token block together, run
   `scripts/checks/design-tokens.py --write`, and commit every file it wrote
   with them.
