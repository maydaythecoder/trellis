# Icons

Thirty-one original icons for TRELLIS's nouns, 16×16 viewBox, 1.5px stroke, round caps and joins, no fills except a dot where one is the glyph. They follow Lucide's grammar on purpose, so a Lucide icon can stand in for anything this set lacks without looking foreign.

Inline the SVG and the stroke takes `currentColor`: `ink-muted` at rest beside secondary text, `ink` beside headings, the status colour inside an Alert or Status, `signal` for the active nav item. Through `<img>` they render in `currentColor`'s fallback (black) — inline them, or set the colour in a wrapper that the SVG inherits.

Sizes: 16px inline with `body-sm` and `meta`; 20px in panel heads and buttons (scale the SVG, keep the 1.5px stroke so it reads as 1.2px — intended); 12px inside keycaps and badges only for `ok`, `error`, `warning`, `run`, `pause`, `stop`.

| icon | means |
| --- | --- |
| `agent`, `subagent` | an agent card; a child agent (the hook off its parent) |
| `task`, `task-done` | a task row; completed |
| `file`, `diff`, `commit`, `trail` | a file node; a change (+/−); a commit; the audit rail |
| `context` | the context meter |
| `model-local`, `model-escalated` | open-weight on this machine; a frontier API (cloud, up-arrow) — always shown when escalated |
| `queue` | the command queue |
| `run`, `pause`, `stop`, `resume` | run controls |
| `budget` | the run budget gauge |
| `call-graph`, `blast-radius` | the repo graph; the radius of a change |
| `vault`, `settings`, `search` | the note vault; settings; search |
| `ok`, `warning`, `error` | status, always beside a word |
| `keyboard`, `terminal`, `desktop` | shortcuts; the TUI face; the desktop face |
| `play` | the playful register (a spark) |
| `burrow-in`, `burrow-out` | go a level deeper (⤢); come back out |

In the terminal face use the glyphs the brand book lists instead (● ○ ◆ ▲ ⤢ ⏎ …); these files are for the desktop face and docs.
