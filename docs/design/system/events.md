<!-- Vendored 2026-10-25 from the owner's TRELLIS design system. Design intent: the events here are the design system's names; what `trellis rpc` actually emits is crates/trellis-cli/tests/client_contract.json. -->

# Events to marks

What the harness emits, and the one visual change each event makes. Both faces implement this table; the desktop may add motion within the budgets, the terminal flashes one frame. Nothing on screen changes without an event, and no event is drawn twice.

| event kind | payload | the mark |
| --- | --- | --- |
| `session.start` | repo, model, resumed? | Shell opens at Overview; a resumed session opens the audit Rail first with the last commit in `signal`. |
| `agent.spawn` | id, parent, model | A new Agent card appears; its frame is `play` for one beat, then `frame`. An Edge from parent to child draws in `edge`. |
| `agent.state` | id, state | The card's Status word and tone change (idle · thinking · calling tool · waiting · blocked · done). Thinking gets the `live` dot. |
| `tool.call` | agent, tool, target | The agent's state goes `cool`; the Edge to the target Node lights `edge-cool`; the target Node's glyph turns `cool`. |
| `tool.result` | agent, tool, ok? | The edge fades back to `edge`; on failure the Node frame turns `frame-down` until the next event touches it. |
| `graph.edge` | from, to, kind | A call-graph Edge is added (orthogonal for calls, dashed for imports); the two Nodes' ports fill. |
| `context.update` | agent, used, budget | The agent's context Meter moves; tone `cool` → `signal` at 70% → `down` at 90%. |
| `context.compact` | agent, before, after | The Meter drains from before to after over 240ms (one cell per 60ms in the terminal). |
| `diff.proposed` | file, hunks | The file's Node gets a `signal` glyph; the Diff appears at Source level with added lines `diff-add`, removed `diff-del`. |
| `diff.applied` | file, commit | The Diff's hunks lose their soft backgrounds; the Rail gains a row with the commit hash. |
| `task.open` / `task.done` | id, title | A row appears on the task Rail; done turns its dot `ok`. When the last open task closes, one `play` face for 2s. |
| `user.needed` | agent, question, keys | The agent goes `warn`; a Keycap row for the answer appears in its card and in the status bar. |
| `tokens.spend` | agent, model, in, out, cost | The Overview spend Stat updates with a `delta`; an escalation adds the `cool` "escalated" tag to the agent. |
| `audit.commit` | hash, summary | A Rail row, newest last, hash in `meta`. The row is `ink` until the next commit, then `ink-secondary`. |
| `error` | agent?, message | A `down` Panel frame on whatever the error belongs to, the message in `down` at `body-sm`; never a modal. |
| anything else | — | A Rail row in `ink-muted` labelled with the kind. |

Rules: one event, one mark. A mark may fade, never disappear without an event. When many events arrive at once (a burst from several agents), draw them in order within the frame budget and let the Rail be the record — the user can always scroll the Rail to see what the screen moved too fast to show.
