# Budget

The run's limits as something you can move, not just read: one row per limit — turns, wall clock, tokens, spend — each a Meter (`cool` → `signal` past 70% → `down` past 90%) with "used / limit", a `+step` button to extend it by one sensible increment, and, before a run or after `edit limits`, an input to set the limit outright. A note says when a change takes effect (from the next turn) and that the trail records who moved it.

Consumer provides: `items` with `used`, `max`, `step`, a `format`, and `onChange(key, newMax)` wired to the harness's set-budget command (a `prompt` field before the run; a budget command during it). `running` decides whether inputs show by default. Never show a figure the harness did not report — leave the row out rather than print 0. The cost row is in the user's currency and is never hidden behind a settings page.
