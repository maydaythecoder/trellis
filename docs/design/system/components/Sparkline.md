# Sparkline

One series as a 1.5px line with a faint area (12% of the tone) and a dot on the last point; no axes, no labels. An optional dotted `baseline` in `line` marks a target or last period. Tones: `signal` (the live series), `cool` (a comparison), `up`/`down` (match the delta), `ink` (on a `cool-fill` tile).

Consumer provides: `values` (2 or more numbers, oldest first), `width`/`height` (default 120×32), `tone`, and an `aria-label` via `label` when the default "N values, last X" isn't enough. Scale is per-sparkline (min to max); when two sparklines must be compared, pass the same `baseline` and say so in the footnote. It draws once; it does not animate on data change.
