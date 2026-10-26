<!-- Vendored 2026-10-25 from the owner's TRELLIS design system (artifact version 1792949950-b51b). Values change in tokens.json first; see ../DESIGN.md §3. -->

> **In this repository.** This is the brand book as the owner wrote it. `docs/design/DESIGN.md` adds the engine's rules and wins where the two differ (its §2.1 lists each difference). The React components the book names are reference designs: the desktop implements them as plain ES modules and the TUI in Go, under `desktop/CLAUDE.md` and ADR 0052. Paths in the text below are the design system's own layout: `assets/Icons/` is `icons/` here, `assets/Logos/` is `logos/`, and `components/<Name>/README.md` is `components/<Name>.md`.

TRELLIS is a local-first agent harness, and this is its design system. TRELLIS has two faces — a terminal UI (Go, Bubble Tea) and a desktop view — driven by one event stream, and this system describes both: every token has a pixel value and a terminal cell or colour-index equivalent, every component says how it draws in each face. The idea both faces serve: the user is the conductor. Agents generate a lot of activity; the interface shows all of it, organised so the user sets the tempo. There are no tabs: a screen is an overview you burrow into, from a headline number down to a row, down to the line of code that produced it, and back out again. The ground is near-black, panels are one hairline off it, type is a monospaced face for anything measured and a plain sans for anything read. Lines connect things. ASCII is a graphic. The mark is the lattice in `assets/Logos/` — six lines, nine crossings, one lit — with a wordmark in the mono face and app icons cut from it. Two accents do the work — a hot amber (`signal`) for what is live or actionable and a cool blue (`cool`) for comparison and flow — and a third, lime (`play`), is allowed to be silly. The content underneath stays exact; only the register changes.

Sections: this brand book, then **Two faces** (how the TUI and desktop render the same model) and **Events to marks** (what each harness event changes on screen).

## Principles

- **Burrow, don't tab.** Every view is an overview with depth behind it. Zoom levels, in order: *Overview* (tiles of Stats and Panels) → *Panel* (one panel fills the stage, its rows visible) → *Row* (one record, its fields and history) → *Source* (the query, code or log that made it). The Trail at the top always shows where you are and the way back out. Nothing is a sibling page you switch to; it is a level you go into.
- **Lines connect, frames contain.** When two things relate, draw the line — an Edge between Nodes, a hairline from a Stat to the Panel it summarises, a timeline rail down a list. Tabs, carousels and segmented controls hide relationships; connectors show them.
- **A conductor, not a filter.** Don't hide data to make a screen calm. Rank it: numerals first, labels second, metadata third, texture last.
- **One hot thing per view.** `signal` marks the single live, selected or actionable element — including the one edge you are following. Two amber things means one of them is wrong.
- **Change is visible.** Every value that can move gets a `delta`, a Sparkline or a Meter. Static numbers are suspicious.
- **ASCII is a first-class graphic.** Box-drawing frames, block-character meters, bracketed keys, `(•_•)` empty states. It renders anywhere, scales with the type, and reads as both precise and friendly.
- **Register is a dial, not a mode.** Serious and unserious use identical components. The dial moves the copy, the ASCII art, and how much `play` is on screen. Nothing structural changes.

## Content fundamentals

- Panel titles and tile labels are uppercase `label-caps`: "TOTAL BALANCE", "DEVICE HEALTH". Everything else is sentence case.
- The Trail reads as a path with `⟩` separators and the current level last in `ink`: "Overview ⟩ Flight delays ⟩ ZH1435 ⟩ query.sql". Each earlier segment is a way back.
- Metadata is a dotted line in `meta`: "1h ago · 44 words · 6 replies". Middle dots, not pipes.
- Numbers are tabular mono with the unit after a `space-1` gap: "1.8 / 6H". Deltas are signed and short: "+2.56%", "−14".
- Keyboard hints bracket the key in prose — "[P] Poke", "[ESC] Up a level" — and use a Keycap in UI. `ESC` always means "out one level"; `⏎` always means "in".
- Status is a word with a dot: "● Online". Never a dot alone.
- **Serious register**: terse and operational. "Clear editor autosave when publishing." No exclamation marks.
- **Playful register**: still short, but allowed a face and a wink. Empty state: `(•_•)  nothing here yet`. Finished run: `\(^o^)/  all 212 passed`. Loader: `[>    ]` ticking across. Faces and glyphs come from the mono face; no emoji in either register — emoji break the grid and the monochrome.

## Visual foundations

### Colour

- Surfaces: `ground` for the page, `panel` for every panel and tile, `panel-raised` for hover and the active tile, `panel-inset` for chart beds, code and ASCII art. The step between each is a hairline's worth; the `frame` shadow (= `line`) draws the edge.
- Text: `ink` for numerals and headings, `ink-secondary` for labels and body, `ink-muted` for metadata. All three hold 4.5:1 on every surface in both themes.
- `signal` (amber): the primary button fill with `ink-inverse` text, the current Trail segment's underline, the selected row's text on `signal-soft`, the live series, the focus ring, the followed edge (`edge-active`). Hover goes to `signal-hover`.
- `cool` (blue): links, the second series, the comparison value, `up` deltas, a second highlighted path (`edge-cool`). `cool-fill` is the only large solid block allowed — one featured tile or the current bar.
- `play` (lime): ASCII faces, loaders, a celebration, the hover glow on a toy. Never a status, never an action. In the serious register it may not appear at all.
- Change and status: `up` blue, `down` orange, `ok` green — three hues that also differ in lightness. Each always travels with a sign, word or icon.
- Connectors: `edge` at rest, `edge-active` for the path under the cursor or selection, `edge-cool` for a second path. Ports (the dots where an edge meets a node) take the edge's colour.
- Charts: `series-1` to `series-5` in order; two is the default, five the ceiling. Chart beds are `panel-inset` with a `grid-dot` grid at `space-6` pitch.
- Daylight theme: same roles on paper (`ground` #f2f1ec); accents darken to hold contrast. Shadows stay frames.

### Type

- JetBrains Mono (Google Fonts, 300–600) for every numeral, label, keycap, timestamp, connector label, line of code and piece of ASCII art. Tabular numerals on.
- IBM Plex Sans (Google Fonts, 400–600) for headings and prose only.
- Numerals are light (300) at 36px and above. `numeral` and `delta` return to 400–500.
- Scale: `numeral-xl` 56 · `numeral-lg` 36 · `numeral` 20 · `delta` 12 · `label-caps` 11 caps/0.08em · `meta` 12 · `keycap` 11 · `code` 13 · `heading` 18 · `heading-sm` 14 · `body` 14 · `body-sm` 13.
- ASCII art is set in `code` with `line-height: 1.25` and `white-space: pre`; a frame drawn in box characters (`┌─┐│└┘`) is a legitimate alternative to a Panel frame inside `panel-inset`.

### Depth and navigation

- The Trail is the only persistent navigation: left-aligned at the top of the stage, `meta` size, segments in `ink-secondary`, the current one in `ink` with a 1px `signal` underline. A zoom indicator at its right ("L2 / 4") says how deep you are.
- Going in: `⏎` on a focused panel or row, or the `⤢` affordance in a Panel head. The panel grows to fill the stage; its siblings compress to a 32px rail on the left with their titles rotated, still clickable.
- Going out: `ESC`, or any earlier Trail segment. The level you leave keeps its scroll and selection.
- Every level shows the same chrome — Trail, status bar, the rail. What changes is the stage.
- Source is the floor. It is a Panel with `inset` on, the code in `code`, and the lines that produced the current number in `signal-soft`.

### Lines, grids, connectors

- Panels tile on a `space-6` (24px) gutter over `ground`; inside, pad `space-3`, or `space-4` on tiles wider than 320px.
- Canvases (graphs, flows, maps) draw a `grid-dot` grid at `space-6` pitch behind content. The plus-grid variant is for empty canvases.
- Edges are 1px, orthogonal by default (two bends max), curved only when a flow is continuous data; they leave and enter a Node at a 4px port dot. Edge labels are `meta` on a `ground` pill so the line passes behind them.
- A rail — a 1px vertical `line` with 6px dots per event — runs down any list that is a sequence in time.
- Corner marks (four 8px L-shapes in `line-strong`) mark the focused group or a selection rectangle. Not on every panel.

### Borders, radii, depth

- Frames are 1px: `frame` at rest, `frame-strong` on hover, `frame-signal` when focused or selected, `frame-down` on error. Never a drop shadow.
- Radii: `radius-0` on panels and tiles, `radius-xs` on controls and keycaps, `radius-node` (6px) on Nodes and ASCII art frames — the one friendly radius — `radius-md` only on dialogs and the outer window, `radius-full` on dots, ports and gauges.
- Depth is ordering: a popover is `panel-raised` with `frame-strong` over a `scrim`; nothing floats.

### States and motion

- Focus: `frame-signal` plus a 1px offset (`outline: 1px solid var(--signal-ring); outline-offset: 1px`). On a panel, the whole frame turns `signal`.
- Hover: surface steps up one; frame steps to `frame-strong`; an edge under the cursor and its two ports turn `edge-active`.
- Selected: `signal` text on `signal-soft`, with a 2px `signal` bar on the leading edge of a row.
- Disabled: `ink-muted` text, frame unchanged, no hover.
- Motion: values tween over 240ms; frames and fills switch instantly; a zoom-in grows the panel over 200ms with siblings compressing in the same beat. An ASCII loader ticks at 120ms per frame — the one thing allowed to loop.

## Iconography

- The icon set lives in `assets/Icons/`: 31 original 16px outline icons for TRELLIS's nouns (agent, subagent, task, diff, context, model-local, model-escalated, queue, run/pause/stop/resume, budget, commit, trail, call-graph, blast-radius, vault, burrow-in/out…), 1.5px stroke, `currentColor` when inlined. They follow Lucide's grammar (ISC), so a Lucide icon stands in for anything the set lacks; 12px inside keycaps and badges.
- Prefer glyphs the mono face already has: ↑ ↓ → ⟩ ⤢ ⌘ ⏎ ⎋ ▲ ▼ ● ○ ◆ █ ░ ┌ ─ ┐ │ └ ┘. A Meter is `█` and `░`; a tree is `├─` and `└─`.
- Faces for the playful register: `(•_•)`, `(•‿•)`, `(ಠ_ಠ)`, `\(^o^)/`, `¯\_(ツ)_/¯`. Pick one per screen, never a zoo.

## TRELLIS vocabulary

- **Agent** states and their Status tones: *idle* → `idle`; *thinking* → `signal` with a `live` dot; *calling tool* → `cool` (the tool's edge lights `edge-cool`); *waiting on you* → `warn` plus a Keycap for the answer; *blocked* → `down`; *done* → `ok`; a subagent spawning → `play` for one beat, then its parent's tone. An agent is drawn as an Agent card: name, model, state, a context Meter and the current tool.
- **Context** is a Meter: `cool` filling, `signal` past 70%, `down` past 90%; the readout is tokens used / budget. Draining (compaction) animates the bar down over 240ms — one of the few things worth watching.
- **Tasks** are rows on a Rail; **files** are Nodes in the call graph; **code** is a Diff at the Source level. Burrow order: Overview (Stats: agents active, tasks open, context used, tokens spent) → Agent or Task → Files touched → Diff.
- **The audit trail** is a Rail of committed events, newest last, each with its commit hash in `meta`. Resuming a session opens here.
- **Models**: an open-weight model is the default and gets no badge; an escalation to a frontier API is a `cool` Status tag ("● escalated · claude") on the agent and the rail event — escalation costs the user money, so it is always visible, never alarming.
- **Tokens brokered**: spend is a Stat with a `delta` per session, in the user's own currency, never hidden in a settings page.

## Components

Twenty families. **Structure**: Panel, Trail, Rail, Shell. **Run**: NowStrip, Transcript, Queue, Budget, Plan — and RunView, the page that composes them. **Data**: Stat, Sparkline, Meter, Status. **Agents and code**: Agent, Diff. **Graph**: Node, Edges. **Controls**: Button, Keycap. **Play**: Ascii. Each ships as a React component on `window.Trellis` with styles in `components/bundle.css`. Panel is the container; Trail is the navigation; Rail is time; Node + Edges make a canvas; Agent and Diff are TRELLIS's own nouns; Shell is the chrome; RunView is the Run view whole — what is running, what is queued, who did what, what it costs, and the code — and burrows from overview to diff; Ascii carries the register. Read each component's README before use.
