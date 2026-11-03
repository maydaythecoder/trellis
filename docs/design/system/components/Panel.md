# Panel

The container everything else sits in: `panel` fill, a 1px `frame`, `radius-0`, and an optional head with an uppercase `label-caps` title on the left and `meta`/actions on the right over a `line` rule. `selected` turns the frame `signal` — one per view, it says "this is where the keyboard is". `corners` adds four 8px marks outside the frame for the focused group or a selection; don't put them on every panel. `inset` puts the body on `panel-inset` for chart beds, code and ASCII. `onOpen` adds the `⤢ ⏎` affordance in the head — this panel can be burrowed into — and `level` prints where it sits (L1 overview … L4 source).

Consumer provides: `children`, and usually `title` and `meta` (a timestamp, a count, a Status). Tile panels on a `space-6` gutter over the dot-grid `ground`. Panels never nest more than one level; a sub-region inside a panel is a `line` rule, not another frame.
