package model

import (
	"strings"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The transcript is rows typed by role (#849), not strings: every place
// that writes it says what the row is, through push or pushRow below, and
// the view paints the role (view/paint.go). The markers stay in the text
// ("> " the person, "· " a note or a call, "! " a refusal), since they
// still read on a terminal with no colour.
//
// The role a row takes:
//   - RoleYou: what the person typed, echoed, and what needs them (`model
//     asks you`); a task and a steer also carry their label, `you · task`
//     or `you · steer`, and the left bar (Row.Bar, pushYou), as the
//     Transcript card draws the person's entries;
//   - RoleLabel: a turn's divider, its number and figures (view.TurnDivider);
//   - RoleTool: a turn's call, whatever the call says;
//   - RolePassed or RoleFailed: a check row, `✓`/`✗ <kind> passed/failed ·
//     trellis-checks verdict`, written only when the turn carried a
//     verdict trellis-checks produced (ADR 0004);
//   - RoleOutput: what the call returned, under it;
//   - RoleText: the model's words, and this view's answers to a /command;
//   - RoleNote: what this view or trellis says about the run or the line:
//     queued, delivered, finished, stopping, a WARN line;
//   - RoleRefusal: an error or a refusal.

// push adds one row of role r.
func (m *Model) push(r view.Role, text string) {
	m.pushRow(view.Row{Text: text, Role: r})
}

// pushYou adds one of the person's own entries: in signal, under the bar.
func (m *Model) pushYou(text string) {
	m.pushRow(view.Row{Text: text, Role: view.RoleYou, Bar: true})
}

// pushRow adds one whole row, its text cut past LineCap bytes (caps.go),
// and keeps the transcript's cap. Every transcript row enters here.
func (m *Model) pushRow(row view.Row) {
	row.Text = capLine(row.Text)
	m.Transcript = append(m.Transcript, row)
	if over := len(m.Transcript) - TranscriptCap; over > 0 {
		m.Transcript = append([]view.Row(nil), m.Transcript[over:]...)
		// The echo awaiting classify's answer moved up, or left the cap.
		m.echoAt = max(m.echoAt-over, 0)
	}
}

// pushText adds served text as rows of role r, skipping blank lines:
// spacing lines carry nothing and would push what does off the screen.
func (m *Model) pushText(r view.Role, s string) {
	for _, l := range strings.Split(s, "\n") {
		if strings.TrimSpace(l) == "" {
			continue
		}
		m.push(r, view.Plain(l))
	}
}
