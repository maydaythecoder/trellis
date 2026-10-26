# Status

A 6px dot and a word in `delta`-weight mono. Tones: `ok` (green — online, passed), `down` (orange — failed, offline), `warn` (amber — check), `signal` (amber — live, selected), `cool` (blue — syncing, info), `idle` (muted — paused, unknown). `tag` adds the tone's soft background; `live` adds a halo for a streaming value.

Consumer provides: `children` — the word, always ("Online", not a bare dot). The three outcome tones differ in lightness as well as hue, so a reader who can't separate green from orange still has the word and the brightness. Status goes in Panel heads, table cells and the sidebar; it never carries a click.
