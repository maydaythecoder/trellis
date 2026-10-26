# Diff

A unified diff for one file at the Source level: file name and +/− counts in the head, `@@` hunk headers in `diff-hunk` on `cool-soft`, added lines in `diff-add` on `diff-add-soft`, removed in `diff-del` on `diff-del-soft`. Green and orange, never green and red — the pair differs in lightness as well as hue. Line numbers sit in two `ink-muted` gutters. `applied` drops the soft backgrounds and shows the commit hash: the change is history now.

Consumer provides: `file`, `hunks` (header + lines with type, old/new numbers, text), `applied`/`commit` once a `diff.applied` event arrives. Show one file per Diff; a change across files is a Rail of files, each burrowing to its Diff. In the terminal the same lines render with `+`/`-` in the sign column and the colour on the text only.
