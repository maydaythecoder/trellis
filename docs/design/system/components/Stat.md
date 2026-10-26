# Stat

A measured value: `label-caps` label, a light `numeral-xl` (56px) or `numeral-lg` (36px, `size="sm"`) value, an optional unit at `meta` size, a signed `delta` in `up`/`down`, and an inline Sparkline when `values` are passed. `featured` sets the tile on `cool-fill` — at most one per view, for the number the screen is about.

Consumer provides: `label`, a pre-formatted `value`, and whatever is known of `unit`, `delta`, `footnote`, `values`. Put a Stat inside a Panel (or straight on `panel` in a tile grid). A Stat without a delta or sparkline should be rare — if the number can't change, it probably belongs in a table.
