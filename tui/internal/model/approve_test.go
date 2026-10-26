package model

import (
	"fmt"
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

// An approval_request opens a modal holding the question and the detail
// apart, as tui_run.rs sends them since #640; y sends approve {id, yes:
// true} and n sends yes: false, each for the id trellis sent, and the
// answer goes back to trellis, the asker.
func TestApproveYesAndNoSendTheirAnswer(t *testing.T) {
	for _, c := range []struct {
		key rune
		yes bool
		say string
	}{{'y', true, "approved"}, {'n', false, "declined"}} {
		m := running(t)
		feed(m, approval("approval-7", "edit a.py", "--- a.py\n+++ a.py\n-return 1\n+return 2\n"))
		if m.Modal == nil || m.Modal.Question != "edit a.py" || m.Modal.Diff != "--- a.py\n+++ a.py\n-return 1\n+return 2" {
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
	if m.Modal.Question != "run tests" || m.Modal.Diff != "" {
		t.Fatalf("no detail: modal %+v", m.Modal)
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
	if !strings.Contains(v, "[y] accept this change  [n] reject") || !strings.Contains(v, "detail line") || !strings.Contains(v, "edit [31ma.py?") {
		t.Fatalf("modal not drawn as expected:\n%s", v)
	}
	if strings.ContainsAny(v, "\x07") || strings.Contains(v, "\x1b[31m") {
		t.Fatalf("a control character from served text reached the screen")
	}
}

// asked is a running model at 80x24 with an approval of question whose
// detail is n numbered lines.
func asked(t *testing.T, question string, n int) *Model {
	t.Helper()
	m := running(t)
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	var diff []string
	for i := 1; i <= n; i++ {
		diff = append(diff, fmt.Sprintf("+ line %02d", i))
	}
	feed(m, approval("a1", question, strings.Join(diff, "\n")))
	return m
}

func press(m *Model, k tea.KeyType) { m.Update(tea.KeyMsg{Type: k}) }

// #640: the arrows and PgUp and PgDn scroll the diff while the modal is
// up, never above its first line or past its last page (#843: a diff that
// fits has no page past its first, so it does not scroll); q, Esc and Enter
// still do nothing, and only y or n answers.
func TestApproveModalScrollStaysInsideTheDiff(t *testing.T) {
	m := asked(t, "edit a.py (+3 -0)", 3)
	press(m, tea.KeyUp)
	press(m, tea.KeyPgUp)
	if m.Modal.Scroll != 0 {
		t.Fatalf("scrolled above the first line: %d", m.Modal.Scroll)
	}
	press(m, tea.KeyPgDown)
	press(m, tea.KeyDown)
	if m.Modal.Scroll != 0 {
		t.Fatalf("scroll %d, want 0: the diff fits, so its first page is its last", m.Modal.Scroll)
	}
	key(m, 'q')
	press(m, tea.KeyEsc)
	press(m, tea.KeyEnter)
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("y"), Paste: true})
	if out := m.TakeOutbox(); len(out) != 0 || m.Quitting || m.Modal == nil || m.Modal.Scroll != 0 {
		t.Fatalf("a key that is not the modal's acted: out %v quitting %v modal %+v", kinds(out), m.Quitting, m.Modal)
	}
}

// #640: at 80x24 a 60-line diff no longer pushes the question out of the
// box. The question is on the top border, the keys on the bottom one, and
// a row says which lines are in view; paging down reaches the last line,
// the question still drawn, and y still answers.
func TestApproveModalKeepsTheQuestionDrawnWhileTheDiffScrolls(t *testing.T) {
	m := asked(t, "edit a.py (+60 -0)", 60)
	v := m.View()
	for _, want := range []string{" edit a.py (+60 -0)? ", "[y] accept this change  [n] reject", "+ line 01", "lines 1–", " of 60"} {
		if !strings.Contains(v, want) {
			t.Fatalf("%q not drawn:\n%s", want, v)
		}
	}
	if strings.Contains(v, "+ line 60") {
		t.Fatalf("the tail is drawn before it is scrolled to:\n%s", v)
	}
	for i := 0; i < 10; i++ {
		press(m, tea.KeyPgDown)
	}
	v = m.View()
	for _, want := range []string{"+ line 60", "–60 of 60", " edit a.py (+60 -0)? "} {
		if !strings.Contains(v, want) {
			t.Fatalf("%q not drawn after paging down:\n%s", want, v)
		}
	}
	key(m, 'y')
	out := m.TakeOutbox()
	if m.Modal != nil || len(out) != 1 || out[0].Cmd["yes"] != true {
		t.Fatalf("y did not answer: %v", kinds(out))
	}
}

// #843: the model let the offset run to the diff's last line while the
// view stopped a page earlier, so after paging to the end the first Up
// moved nothing. The offset now stops where the view does.
func TestApproveModalUpMovesAtOnceAfterTheEnd(t *testing.T) {
	m := asked(t, "edit a.py (+60 -0)", 60)
	for i := 0; i < 10; i++ {
		press(m, tea.KeyPgDown)
	}
	end := m.View()
	if !strings.Contains(end, "–60 of 60") {
		t.Fatalf("paging down did not reach the end:\n%s", end)
	}
	press(m, tea.KeyUp)
	if up := m.View(); up == end || strings.Contains(up, "–60 of 60") {
		t.Fatalf("the first Up after the end did not move the view:\n%s", up)
	}
}

// A last line that wraps to more rows than the box shows is its own last
// page: the offset stops at that line and the box shows what fits of it.
// The limit ran one past the last line, and one Down on a long bash
// command emptied the box and read "lines 2–2 of 1", leaving nothing of
// the call being answered on screen.
func TestApproveModalALineTallerThanTheBoxIsItsOwnLastPage(t *testing.T) {
	for _, c := range []struct {
		diff, shows string
		last        int
	}{
		{strings.Repeat("x", 900), "xxxxxxxx", 0},
		{"echo start\n" + strings.Repeat("y", 900), "yyyyyyyy", 1},
	} {
		m := running(t)
		m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
		feed(m, approval("a1", "bash", c.diff))
		for i := 0; i < 5; i++ {
			press(m, tea.KeyPgDown)
			press(m, tea.KeyDown)
		}
		if m.Modal.Scroll != c.last {
			t.Fatalf("scroll %d, want %d: the last line is as far as it goes", m.Modal.Scroll, c.last)
		}
		n := c.last + 1
		if v := m.View(); !strings.Contains(v, c.shows) || !strings.Contains(v, fmt.Sprintf("lines %d–%d of %d", n, n, n)) {
			t.Fatalf("the box does not show the last line from its start:\n%s", v)
		}
	}
}

// A gated call that is not an edit is named a call, and a diff that fits
// says no position.
func TestApproveModalNamesACallThatIsNotAnEdit(t *testing.T) {
	v := asked(t, "bash", 3).View()
	if !strings.Contains(v, " bash? ") || !strings.Contains(v, "[y] accept this call  [n] reject") {
		t.Fatalf("view:\n%s", v)
	}
	if strings.Contains(v, "lines 1–") {
		t.Fatalf("a diff that fits says a position:\n%s", v)
	}
}
