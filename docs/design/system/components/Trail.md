# Trail

The only navigation in the system: the path from Overview to where you are, and the way back out. Segments are `meta`-size mono in `ink-secondary`; the current one is `ink` with a 1px `signal` underline; `⟩` separates them. On the right, a tick gauge and "L2 / 4" say how deep you are, and an `ESC out` keycap says how to leave.

Consumer provides: `segments` (Overview first, current last) and `onGo(index)` to jump back to an earlier level. It sits at the top of the stage on every level and never scrolls away. There are no tabs anywhere in this system — if you find yourself wanting one, the two things are either levels (Trail) or siblings in a rail.
