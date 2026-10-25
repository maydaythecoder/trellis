# Design

The design philosophy and system, the wireframes, and the notes behind them.
A wireframe here is a target for the two rpc clients (`desktop/`, `tui/`), not
a picture of the product as built; the pictures of the product are in
`docs/assets/`.

| File | What it is | Ticket |
|---|---|---|
| `DESIGN.md` | What the interface is for (the conductor, change as the event, one view that burrows inward, lines and glyphs, two faces), the rules it keeps, and the system: colour meanings and values, type, space, radius, regions. Start here. | #786 |
| `run-view-wireframe.html` | The run view, second pass: one self-contained page, opens from disk. Levels `run`, `code`, `summary`, `tui`; states `idle`, `running`, `waiting`, `paused`, `stopping`, `converged`, `not_converged`, `interrupted`, `error`; dark and light. | #786 |
| `run-view-research-2026-10-25.md` | What sixteen agent harnesses and the oversight literature do for the same regions, with sources. Informs the wireframe; decides nothing on its own. | #786 |

## Reading the wireframe

Open the file in a browser. The dashed box at the top right is the wire
control: pick a state, a phase (while running), a level, the theme, and
`sources`. The same choices work as query parameters, e.g.
`run-view-wireframe.html?state=waiting&wait=approve&level=code`.

`sources` is the part that matters for tickets. With it on, every region ends
in a tag naming the rpc event or command it draws from. A green tag is reported
today; `NEW` inside a tag, or a red tag, is not built and names the command or
event it would need. The follow-up tickets are that list.

What the page never shows, by rule: a currency figure (spend is not metered
under the Local regime), a percent-complete bar, a remaining-time estimate, a
sum of `Verified` and `Reviewed`, a radius ratio, or a verdict the client
computed. A figure trellis did not report reads `unreported`, never `0`.

The sample run is one loop with one model, which is v1 scope. Subagents are
deferred and do not appear.

## Changing it

The page is the Trellis design tokens (`:root` and `[data-theme=light]`), the
component styles and bundle, then `v2` styles and the page script, each in its
own `<style>` or `<script>` block in that order. Edit the last two. Keep every
colour, length and radius a token. A change to the token block is followed by
`scripts/checks/design-tokens.py --write`, which regenerates the value tables
in `DESIGN.md`; CI fails when the two drift. The rules an agent follows when
touching the clients are in `.claude/skills/trellis-ui/SKILL.md`.
