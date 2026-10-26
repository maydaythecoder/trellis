# Agent

One agent as a card body: `◆ name` (`└ name` for a subagent) with its state as a Status on the right, the model line in `ink-muted` (plus "● escalated" in `cool` whenever a frontier API is in use — never hidden), the current tool and target, and a context Meter (`cool` → `signal` past 70% → `down` past 90%). A `waiting` agent shows the keys that answer it.

Consumer provides: `name`, `model`, `state` (idle · thinking · tool · waiting · blocked · done · spawning), `contextUsed`/`contextBudget`, and `tool`/`target` while a tool runs; `parent` for a subagent, `keys` when waiting. Put it inside a Panel titled with the agent's task; on the Overview, several Agents tile at `space-6`. Every state change comes from an `agent.state` event — the card never guesses.
