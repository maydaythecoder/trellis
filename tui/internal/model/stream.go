package model

import (
	"strings"
	"unicode/utf8"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The reply of the turn in progress as colibri streams it (#1059): rpc's
// model token events, one per content delta. They are the model's words
// while it writes, never the turn's result, so they are kept apart from
// the transcript and drawn below its rows until the turn's own event, the
// run's end or the child's exit, when they are dropped and the turn's rows
// stand in their place.

// StreamCap is how many bytes of the reply in progress are kept, the
// newest: a reply of minutes would otherwise grow without bound.
const StreamCap = 16 << 10

// streamLabel heads the streamed rows: what they are, and that they are
// the model's words, not a verdict (trellis-ui rule 1).
const streamLabel = "· model reply so far, streamed; its words, not a verdict"

// modelToken adds one delta to the reply in progress, keeping its newest
// StreamCap bytes, cut at a character's start.
func (m *Model) modelToken(text string) {
	m.streamed += text
	if over := len(m.streamed) - StreamCap; over > 0 {
		for over < len(m.streamed) && !utf8.RuneStart(m.streamed[over]) {
			over++
		}
		m.streamed = m.streamed[over:]
		m.streamCut = true
	}
}

// streamEnded drops the reply in progress: its turn, or the run, has
// ended, and what the step was is the turn's event, not the deltas.
func (m *Model) streamEnded() {
	m.streamed, m.streamCut = "", false
}

// transcriptRows is the transcript as drawn: its rows, then the reply in
// progress, if any, joined into lines rather than a row per delta, each
// made plain since it is model text.
func (m *Model) transcriptRows() []view.Row {
	if m.streamed == "" {
		return m.Transcript
	}
	label := streamLabel
	if m.streamCut {
		label += "; its last 16 KiB"
	}
	rows := append([]view.Row(nil), m.Transcript...)
	rows = append(rows, view.Row{Text: label, Role: view.RoleText})
	for _, l := range strings.Split(m.streamed, "\n") {
		if strings.TrimSpace(l) == "" {
			continue
		}
		rows = append(rows, view.Row{Text: capLine(view.Plain(l)), Role: view.RoleText})
	}
	return rows
}
