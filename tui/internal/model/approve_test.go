package model

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func approval(id, question, detail string) string {
	return jsonLine(map[string]any{"type": "approval_request", "id": id, "question": question, "detail": detail})
}

func key(m *Model, r rune) { m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}}) }

// running is a model with a run in flight and an empty outbox.
func running(t *testing.T) *Model {
	t.Helper()
	m := fresh(t)
	submit(m, "change f")
	m.TakeOutbox()
	return m
}

// An approval_request opens a modal showing the detail, then the question
// with a "?", as tui_run.rs builds it; y sends approve {id, yes: true} and
// n sends yes: false, each for the id trellis sent, and the answer goes
// back to trellis, the asker.
func TestApproveYesAndNoSendTheirAnswer(t *testing.T) {
	for _, c := range []struct {
		key rune
		yes bool
		say string
	}{{'y', true, "approved"}, {'n', false, "declined"}} {
		m := running(t)
		feed(m, approval("approval-7", "edit a.py", "--- a.py\n+++ a.py\n-return 1\n+return 2\n"))
		if m.Modal == nil || m.Modal.Text != "--- a.py\n+++ a.py\n-return 1\n+return 2\nedit a.py?" {
			t.Fatalf("modal %+v", m.Modal)
		}
		key(m, c.key)
		out := m.TakeOutbox()
		if len(out) != 1 || out[0].Cmd["type"] != "approve" || out[0].Cmd["id"] != "approval-7" || out[0].Cmd["yes"] != c.yes {
			t.Fatalf("%c sent %+v", c.key, out)
		}
		if m.Modal != nil {
			t.Fatal("the modal stayed up after its answer")
		}
		if last := m.Transcript[len(m.Transcript)-1]; last != "· edit a.py "+c.say {
			t.Fatalf("last line %q", last)
		}
	}
}

// While the modal is up it takes the keys: only y and n act. q, Enter,
// Esc and a typed line do nothing, nothing is sent and the line is not
// edited, so no key meant for the question becomes a steer or a quit.
func TestApproveModalTakesTheKeys(t *testing.T) {
	m := running(t)
	feed(m, approval("a1", "run tests", ""))
	if m.Modal.Text != "run tests?" {
		t.Fatalf("no detail: modal text %q", m.Modal.Text)
	}
	key(m, 'q')
	typeLine(m, "x z")
	for _, k := range []tea.KeyType{tea.KeyEnter, tea.KeyEsc, tea.KeyBackspace, tea.KeyTab} {
		m.Update(tea.KeyMsg{Type: k})
	}
	if out := m.TakeOutbox(); len(out) != 0 || m.Quitting || m.Input != "" || m.Modal == nil {
		t.Fatalf("a key under the modal acted: out %+v quitting %v input %q modal %v", out, m.Quitting, m.Input, m.Modal)
	}
}

// A second question waits behind the first, and each answer goes to its
// own id, in order.
func TestApproveQuestionsAreAnsweredOneAtATimeByTheirOwnID(t *testing.T) {
	m := running(t)
	feed(m, approval("a1", "first", ""))
	feed(m, approval("a2", "second", ""))
	key(m, 'n')
	if m.Modal == nil || m.Modal.ID != "a2" {
		t.Fatalf("the second question is not up: %+v", m.Modal)
	}
	key(m, 'y')
	out := m.TakeOutbox()
	if len(out) != 2 || out[0].Cmd["id"] != "a1" || out[0].Cmd["yes"] != false || out[1].Cmd["id"] != "a2" || out[1].Cmd["yes"] != true {
		t.Fatalf("answers %+v", out)
	}
}

// Leaving mid-question never says yes: ^c closes the modal unanswered and
// sends abort then quit, so trellis answers its gate no and rolls back.
func TestApproveLeavingMidQuestionSaysNoYes(t *testing.T) {
	m := running(t)
	feed(m, approval("a1", "edit a.py", ""))
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	got := kinds(m.TakeOutbox())
	if strings.Join(got, ",") != "abort,QUIT" || m.Modal != nil {
		t.Fatalf("sent %v, modal %v", got, m.Modal)
	}
	// A run that ends while a question is up takes the question with it.
	m = running(t)
	feed(m, approval("a1", "edit a.py", ""))
	feed(m, runEnd(map[string]any{"outcome": "interrupted", "exit": 130, "restored": true}))
	if m.Modal != nil {
		t.Fatal("a question outlived its run")
	}
}

// The modal covers the screen: none of the transcript under it is drawn,
// and served text in it has its control characters removed.
func TestApproveModalCoversTheScreen(t *testing.T) {
	m := running(t)
	m.Update(tea.WindowSizeMsg{Width: 100, Height: 30})
	feed(m, approval("a1", "edit \x1b[31ma.py", "detail line\x07"))
	v := m.View()
	if strings.Contains(v, "change f") {
		t.Fatalf("the transcript shows through the modal:\n%s", v)
	}
	if !strings.Contains(v, "[y] accept  [n] reject") || !strings.Contains(v, "detail line") || !strings.Contains(v, "edit [31ma.py?") {
		t.Fatalf("modal not drawn as expected:\n%s", v)
	}
	if strings.ContainsAny(v, "\x07") || strings.Contains(v, "\x1b[31m") {
		t.Fatalf("a control character from served text reached the screen")
	}
}
