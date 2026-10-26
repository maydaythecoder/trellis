# Node

A box on a canvas: `panel` fill, `frame-strong`, the one friendly radius (`radius-node`), a `glyph` + title line and an `ink-muted` sub line, with 6px port dots on the left (in) and right (out). `selected` turns the frame and ports `signal`; `tone="play"` frames it in lime for a sandbox, a toy, a wildcard step.

Consumer provides: `title`, `x`/`y` in canvas px (the parent is a `.cd-canvas`, position: relative), and usually `sub` and `glyph` from the mono face (◆ for a transform, ● for a source, ○ for a sink, ▲ for a trigger, `{ }` for code). Edges connect nodes by their port coordinates — the consumer computes those from `x`, `y` and `width`. Nodes are the one place the system is allowed to be a little soft.
