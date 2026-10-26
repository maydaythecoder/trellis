# Keycap

An 18px-tall key in `keycap` type on `panel-raised` with a `frame-strong`; `tone="signal"` highlights the key that is armed. Pass `keys` as one string or an array for a chord, and `label` to render "⌘K Commands" as one unit for the status bar.

Consumer provides: `keys` (use the symbols ⌘ ⇧ ⌥ ⏎ ⎋ ↑ ↓ ← → where they apply, letters uppercase), optional `label`. Keycaps describe shortcuts; they are not buttons and have no handlers. Every action with a visible button also appears somewhere as a Keycap.
