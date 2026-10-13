package model

import (
	"strings"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
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
	// Scroll is the first line of Diff in view. It is held below the line
	// count here; the view stops short of it once the rest fits, since how
	// much fits is layout (as ScrollBack is for the transcript).
	Scroll int
}

// approvalRequested shows the question, or queues it behind the one on
// screen: one question at a time, as the gate asks them. Once leaving has
// been asked for, a question is not shown: the run is stopping, trellis
// drops the question once it has, and a dropped question is a no (#634).
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
	ap := Approval{ID: a.ID, Question: Plain(q), Diff: plainLines(strings.TrimRight(d, " \t\r\n"))}
	if m.Modal == nil {
		m.Modal = &ap
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

// scrollModal moves the modal's first line by d, within the diff: never
// above its first line, never past its last.
func (m *Model) scrollModal(d int) {
	last := len(strings.Split(m.Modal.Diff, "\n")) - 1
	s := m.Modal.Scroll + d
	if s > last {
		s = last
	}
	if s < 0 {
		s = 0
	}
	m.Modal.Scroll = s
}

// answer sends approve {id, yes} for the question on screen, says which,
// and shows the next question waiting, if any.
func (m *Model) answer(yes bool) {
	a := m.Modal
	m.send("approve", map[string]any{"id": a.ID, "yes": yes})
	m.markEdits(yes)
	word := "declined"
	if yes {
		word = "approved"
	}
	m.pushLine("· " + a.Question + " " + word)
	m.Modal = nil
	if len(m.approvals) > 0 {
		next := m.approvals[0]
		m.approvals = m.approvals[1:]
		m.Modal = &next
	}
}

// plainLines keeps the line breaks of served text and removes every other
// control character.
func plainLines(s string) string {
	lines := strings.Split(s, "\n")
	for i, l := range lines {
		lines[i] = Plain(l)
	}
	return strings.Join(lines, "\n")
}
