package model

import (
	"fmt"
	"strings"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// ModalPage is how far PgUp and PgDn move the approval modal's diff: the
// ratatui TUI's MODAL_PAGE, ten of the twelve rows its box has at 80x24.
const ModalPage = 10

// Approval is a question trellis asked before a gated call, in approve
// mode. trellis waits for as long as nobody answers, and only yes: true
// approves (ADR 0018).
type Approval struct {
	// ID is the request's own id: the client answers only an id it
	// received.
	ID string
	// Question names the call; the modal draws it on its top border,
	// where a long diff cannot push it out (#640).
	Question string
	// Diff is the request's detail, line breaks kept.
	Diff string
	// Scroll is the first line of Diff in view, held between the first line
	// and the start of the last full page at the current size, the view's
	// own limit (view.AskingLastFrom, #843).
	Scroll int
}

// approvalRequested shows the question, or queues it behind the one on
// screen: one question at a time, as the gate asks them. Once leaving has
// been asked for, a question is not shown: the run is stopping, trellis
// drops the question once it has, and a dropped question is a no (#634).
// With ApprovalsCap already waiting, the question is declined at once and
// the transcript says so (#848): dropped unanswered, it would hold the run
// forever, and a no changes nothing in the tree.
func (m *Model) approvalRequested(a frames.ApprovalFields) {
	if m.Quitting {
		return
	}
	q, d := "", ""
	if a.Question != nil {
		q = *a.Question
	}
	if a.Detail != nil {
		d = *a.Detail
	}
	ap := Approval{ID: a.ID, Question: view.Plain(q), Diff: plainLines(strings.TrimRight(d, " \t\r\n"))}
	if m.Modal == nil {
		m.showModal(&ap)
		return
	}
	if len(m.approvals) >= ApprovalsCap {
		m.send("approve", map[string]any{"id": ap.ID, "yes": false})
		m.push(view.RoleNote, fmt.Sprintf("· %s declined unseen: %d questions were already waiting", ap.Question, len(m.approvals)+1))
		return
	}
	m.approvals = append(m.approvals, ap)
}

// modalKey is a key while the modal is up. y and n answer it; the arrows
// and PgUp and PgDn scroll its diff, so a long one is read whole before it
// is answered (#640). Every other key, a paste among them, is ignored: the
// keys on screen are the modal's.
func (m *Model) modalKey(k tea.KeyMsg) {
	switch k.Type {
	case tea.KeyRunes:
		if k.Paste || len(k.Runes) != 1 {
			return
		}
		switch k.Runes[0] {
		case 'y':
			m.answer(true)
		case 'n':
			m.answer(false)
		}
	case tea.KeyUp:
		m.scrollModal(-1)
	case tea.KeyDown:
		m.scrollModal(1)
	case tea.KeyPgUp:
		m.scrollModal(-ModalPage)
	case tea.KeyPgDown:
		m.scrollModal(ModalPage)
	}
}

// scrollModal moves the modal's first line by d, never above the diff's
// first line and never past the start of its last full page, the view's
// own limit. Stopped at the diff's last line instead, the offset ran a page
// past what the view shows, and the first Up after the end moved nothing
// (#843).
func (m *Model) scrollModal(d int) {
	w, h := m.size()
	m.Modal.Scroll = max(min(m.Modal.Scroll+d, view.AskingLastFrom(m.Modal.Diff, w, h)), 0)
}

// answer sends approve {id, yes} for the question on screen, says which,
// and shows the next question waiting, if any. The edits pane is left as
// trellis reported it: the answer is about the next call, and an edit the
// loop has written is in the tree whatever the answer (#635).
func (m *Model) answer(yes bool) {
	a := m.Modal
	m.send("approve", map[string]any{"id": a.ID, "yes": yes})
	word := "declined"
	if yes {
		word = "approved"
	}
	m.push(view.RoleNote, "· "+a.Question+" "+word)
	m.closeModal()
	if len(m.approvals) > 0 {
		next := m.approvals[0]
		m.approvals = m.approvals[1:]
		m.showModal(&next)
	}
}

// plainLines keeps the line breaks of served text and removes every other
// control character.
func plainLines(s string) string {
	lines := strings.Split(s, "\n")
	for i, l := range lines {
		lines[i] = view.Plain(l)
	}
	return strings.Join(lines, "\n")
}
