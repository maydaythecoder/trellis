# Queue

The composer and what is waiting behind it. While a run is in flight the composer says what Send will do — `● steer now` (delivered at the agent's next model call) or `○ queue follow-up` (runs after this run as a new prompt) — and the list below shows every pending item numbered, with its kind, its text, when it will fire, and ↑ ↓ ✎ × to reorder, edit or cancel. A delivered steer leaves the list and appears in the Transcript with "delivered after turn N". With no run in flight, Send starts one.

Consumer provides: `running`, `mode` and `onMode`, `items` (pending steers and follow-ups from the harness — `get_state` must report them), and `onSend`, `onMove`, `onEdit`, `onCancel` wired to `steer`, `follow_up` and their cancel/edit commands. `⌘⏎` sends; `S`/`F` switch mode; `Q` focuses the composer from anywhere. The queue is never hidden while it has items.
