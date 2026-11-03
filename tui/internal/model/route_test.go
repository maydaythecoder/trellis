package model

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

// sent types a line, sends it, and returns the classify it became.
func sent(t *testing.T, m *Model, text string) Out {
	t.Helper()
	typeLine(m, text)
	enter(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "classify" || out[0].Cmd["text"] != text {
		t.Fatalf("%q sent %+v, want one classify carrying it", text, out)
	}
	return out[0]
}

// A question is answered in the transcript as text and sends no prompt:
// nothing is snapshotted, run, checked or rolled back for it.
func TestRouteAQuestionSendsNoPrompt(t *testing.T) {
	m := fresh(t)
	c := sent(t, m, "what does f do?")
	respond(m, c.Cmd["id"], "classify", map[string]any{"intent": "question", "answer": "f returns 1.\n\nIt is called from g."})
	if out := m.TakeOutbox(); len(out) != 0 || m.Running {
		t.Fatalf("a question ran: %+v", out)
	}
	if got := strings.Join(texts(m), "|"); got != "you  what does f do?|f returns 1.|It is called from g." {
		t.Fatalf("transcript %q", got)
	}
}

// A task becomes one prompt with the same text, echoed once.
func TestRouteATaskSendsOnePrompt(t *testing.T) {
	m := fresh(t)
	c := sent(t, m, "make f return 2")
	respond(m, c.Cmd["id"], "classify", map[string]any{"intent": "task"})
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "prompt" || out[0].Cmd["task"] != "make f return 2" || !m.Running {
		t.Fatalf("%+v running %v", out, m.Running)
	}
	if strings.Count(strings.Join(texts(m), "\n"), "make f return 2") != 1 || texts(m)[0] != "you · task  make f return 2" || !m.Transcript[0].Bar {
		t.Fatalf("echo %q", texts(m))
	}
}

// A failed classify is reported as tui_run.rs reports one, every line of
// its error marked, nothing runs, and input is handed back.
func TestRouteAFailedClassifyRunsNothing(t *testing.T) {
	m := fresh(t)
	c := sent(t, m, "make f return 2")
	feed(m, jsonLine(map[string]any{"type": "response", "command": "classify", "id": c.Cmd["id"], "success": false,
		"error": "choose a model first\ncaused by: no model"}))
	if out := m.TakeOutbox(); len(out) != 0 || m.Running {
		t.Fatalf("%+v", out)
	}
	tail := strings.Join(texts(m)[1:], "|")
	if tail != "! choose a model first|! caused by: no model|· nothing was run" {
		t.Fatalf("%q", tail)
	}
	sent(t, m, "again")
}

// An answer holding an ESC byte is shown without it.
func TestRouteAnAnswerIsShownPlain(t *testing.T) {
	m := fresh(t)
	c := sent(t, m, "hello")
	respond(m, c.Cmd["id"], "classify", map[string]any{"intent": "question", "answer": "hi\x1b[2J there\x07"})
	for _, l := range texts(m) {
		if strings.ContainsAny(l, "\x1b\x07") {
			t.Fatalf("%q", l)
		}
	}
	if last(m) != "hi[2J there" {
		t.Fatalf("%q", last(m))
	}
}

// While trellis is asked, another line is held back rather than sent, and
// ^c drops the pending classify with abort before quitting; an answer that
// names no known intent runs nothing.
func TestRouteWhileAskingAndAnUnknownIntent(t *testing.T) {
	m := fresh(t)
	c := sent(t, m, "first")
	typeLine(m, "second")
	enter(m)
	if out := m.TakeOutbox(); len(out) != 0 || m.Input != "second" {
		t.Fatalf("%+v input %q", out, m.Input)
	}
	respond(m, c.Cmd["id"], "classify", map[string]any{"intent": "chat"})
	if out := m.TakeOutbox(); len(out) != 0 || m.Running || !strings.Contains(last(m), "no intent this view knows") {
		t.Fatalf("%+v %q", out, last(m))
	}
	m.Input = ""
	sent(t, m, "third")
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	if got := strings.Join(kinds(m.TakeOutbox()), ","); got != "abort,QUIT" {
		t.Fatalf("%s", got)
	}
}
