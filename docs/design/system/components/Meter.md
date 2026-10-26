# Meter

A progress or ratio bar drawn in block characters — `████████░░░░░░░░ 52%` — so it sets in any line of mono text, scales with the type and needs no canvas. Filled cells are `signal` (or the tone), empty cells `line-strong`; the readout is `ink`. An optional uppercase label leads.

Consumer provides: `value` (0–1, or a count with `max`), optional `label`, `cells` (default 16; use 8 in a table cell, 32 in a hero), `tone` (`cool` comparison, `ok` healthy, `down` over budget, `play` for a loader or game), and `format` for a custom readout ("6,000 / 12,000"). Stack Meters at `space-2` for a breakdown. A Meter shows a share; a Sparkline shows a history — don't use one for the other.
