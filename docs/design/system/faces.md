<!-- Vendored 2026-10-25 from the owner's TRELLIS design system. Design intent: the events here are the design system's names; what `trellis rpc` actually emits is crates/trellis-cli/tests/client_contract.json. -->

# Two faces, one model

TRELLIS renders the same event stream twice: in a terminal (Go, Bubble Tea) and in a desktop window. The desktop face is not a prettier version of the terminal; the terminal is not a degraded version of the desktop. They are the same model at two resolutions, and a user who switches between them should recognise every panel, every colour role and every keystroke.

## What is shared

- Layout: the Trail on top, the stage in the middle, the status bar at the bottom, the sibling rail on the left when a panel is open. Same order, same proportions (see the `cells` tokens: 1 column ≈ `space-2`, 1 row ≈ `space-5`).
- Colour roles and their meaning. Truecolor terminals use the hex values; otherwise the `terminal` tokens give the 256-colour index and the 16-colour fallback.
- Type roles: `label-caps` is uppercase in both; `numeral-xl` is 3-row block digits in the terminal and a 56px light numeral on the desktop.
- Keys: `⏎` in, `ESC` out, `⌘K`/`Ctrl-K` commands, `↑↓` navigate, `TAB` select. Identical in both faces; the Keycap shows the platform's glyph.
- Register: the same faces, the same `play` rules.

## What the terminal does with less

- Frames are box-drawing characters in `term-line`: `┌─┐ │ └─┘`. A selected panel redraws its frame in `term-signal`; corner marks become `┏ ┓ ┗ ┛`.
- Edges are `─ │ ╭ ╮ ╰ ╯` (rounded corners when the terminal font has them, `┌ ┐ └ ┘` otherwise); ports are `●`; the followed edge is in `term-edge-active`.
- Sparklines are `▁▂▃▄▅▆▇█` — eight levels, one column per sample. Meters are `█` and `░`, exactly as on the desktop.
- Grids are `·` at every other column, every row, in `term-line-strong`; drop them under 80 columns.
- Motion is frames: a spinner cycles `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` at 120ms; a context drain redraws the Meter one cell per 60ms; a value change flashes the cell in `term-signal` for one frame. Nothing else animates.
- Depth: the burrow replaces the stage; siblings collapse to a 3-column rail with their titles written downward. The Trail is the top row.
- Below 80 columns: one panel at a time, Trail and status bar remain. Below 24 rows: the status bar hides until a key is held.

## What the desktop does with more

- Real 1px lines in `line`/`edge`, curved edges for continuous flow, labels on pills.
- More on screen: the overview shows every agent and the call graph at once; the terminal shows the four headline Stats and a list.
- Smooth motion within the budgets in the brand book: values tween over 240ms, a panel grows into the stage over 200ms, edges light up with a 120ms fade. The desktop may let an idle canvas breathe — ports pulse once every 4s in `edge`, no more.
- Hover exists. It steps a surface up and lights the edge under the cursor; the terminal has no hover and must be as usable without it, so hover never carries information that focus does not.

## The event stream contract (what both faces consume)

Every event has `ts`, `agent`, `kind` and a `payload`; both faces subscribe to the same stream and never derive state from each other. The visual effect of each kind is specified in **Events to marks**. A face that does not know a `kind` draws it as a Rail row in `ink-muted` with its kind as the label — nothing is dropped silently.
