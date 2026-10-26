# Rail

A vertical timeline: a 1px `line` rail with a 6px dot per row, the row's label in `body-sm` and its metadata right-aligned in `meta`. Dots take a tone (`ok` done, `down` failed, `signal` live, `cool` escalated or tool, `play` celebration); the `current` row is `ink` with a `signal` ring on its dot. Rows with an `onClick` are the way to burrow into that moment.

Consumer provides: `rows` — oldest first, newest last (time runs down), each with `label`, `meta` (a time, a hash, a count), optional tone and `current`. The Rail is TRELLIS's audit trail, task list and event log: three uses, one shape. Never truncate it silently; when it is long, the stage scrolls and the Trail stays.
