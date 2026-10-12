package model

import (
	"strings"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
)

// Approval is a question trellis asked before a gated call, in approve
// mode. trellis waits for as long as nobody answers, and only yes: true
// approves (ADR 0018).
type Approval struct {
	// ID is the request's own id: the client answers only an id it
	// received.
	ID       string
	Question string
	// Text is what the modal shows: the detail, then the question.
	Text string
}

// approvalRequested shows the question, or queues it behind the one on
// screen: one question at a time, as the gate asks them.
func (m *Model) approvalRequested(a frames.ApprovalFields) {
	q, d := "", ""
	if a.Question != nil {
		q = *a.Question
	}
	if a.Detail != nil {
		d = *a.Detail
	}
	// As tui_run.rs builds it: the detail, then the question with a "?".
	text := q + "?"
	if strings.TrimSpace(d) != "" {
		text = strings.TrimRight(d, " \t\r\n") + "\n" + q + "?"
	}
	ap := Approval{ID: a.ID, Question: Plain(q), Text: plainLines(text)}
	if m.Modal == nil {
		m.Modal = &ap
		return
	}
	m.approvals = append(m.approvals, ap)
}

// answer sends approve {id, yes} for the question on screen, says which,
// and shows the next question waiting, if any.
func (m *Model) answer(yes bool) {
	a := m.Modal
	m.send("approve", map[string]any{"id": a.ID, "yes": yes})
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
