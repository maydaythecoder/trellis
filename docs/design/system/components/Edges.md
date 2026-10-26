# Edges

SVG connectors drawn over a canvas: 1px `edge` lines, orthogonal with two bends by default, an S-curve (`curve`) for continuous data flow, `dashed` for optional or inferred links. Each edge ends in a 2px port dot. `tone="active"` paints the path you are following in `signal`; `tone="cool"` a second path in blue. A `label` sits on the midpoint on a `panel-inset` pill so the line passes behind it.

Consumer provides: `edges` as `{from:[x,y], to:[x,y]}` in canvas px, plus tone, label, curve, bendAt as needed; render it as the first child of a `.cd-canvas` so Nodes stack above. At most one `active` edge per canvas. When more than ~40 edges share a canvas, fade all but the active one's neighbours to `line`.
