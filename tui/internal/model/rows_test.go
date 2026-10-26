package model

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// texts is the transcript's rows as their text, oldest first.
func texts(m *Model) []string {
	out := make([]string, len(m.Transcript))
	for i, r := range m.Transcript {
		out[i] = r.Text
	}
	return out
}

// #849: every transcript row has the role its writer gave it, and the role
// matches what the row is. A run with a task, a question, a steer, turns
// with and without a verdict, an observation, a WARN line, an approval, a
// refused command, a /command and the run's end writes a row of every
// transcript role, and each is checked against the frame or the key that
// made it. Words never make a role: the model's answer below says "> " and
// "✓ check passed" and stays the model's words, so the model cannot pass
// for the person or for a verdict.
func TestRowsEveryTranscriptLineHasARole(t *testing.T) {
	m := fresh(t)
	q := sent(t, m, "what does f do?")
	respond(m, q.Cmd["id"], "classify", map[string]any{"intent": "question", "answer": "> it returns 1\nsee g  ✓ check passed"})
	submit(m, "make f return 2")
	m.TakeOutbox()
	typeLine(m, "use the helper")
	enter(m)
	feed(m, steerDelivered(0))
	feed(m, turn(1, "read_file f.py", map[string]any{"observation": "def f():\n    return 1"}))
	feed(m, turn(2, "run_checks", map[string]any{"verdict": "Some(true)", "check_kind": "pytest"}))
	feed(m, turn(3, "run_checks", map[string]any{"verdict": "Some(false)", "check_kind": "mypy"}))
	feed(m, warn("context nearly full"))
	feed(m, approval("a1", "edit f.py", "+return 2"))
	key(m, 'y')
	feed(m, jsonLine(map[string]any{"type": "response", "command": "abort", "id": "x", "success": false, "error": "no run"}))
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	typeLine(m, "/help")
	enter(m)
	typeLine(m, "/nonsense")
	enter(m)

	want := map[string]view.Role{
		"> what does f do?":                        view.RoleYou,
		"> it returns 1":                           view.RoleText,
		"see g  ✓ check passed":                    view.RoleText,
		"> make f return 2":                        view.RoleYou,
		"· queued: use the helper":                 view.RoleYou,
		"· delivered after turn 0: use the helper": view.RoleNote,
		"· read_file f.py":                         view.RoleTool,
		"    def f():":                             view.RoleOutput,
		"        return 1":                         view.RoleOutput,
		"· run_checks  ✓ pytest passed":            view.RolePassed,
		"· run_checks  ✗ mypy failed":              view.RoleFailed,
		"context nearly full":                      view.RoleNote,
		"· edit f.py approved":                     view.RoleNote,
		"! abort: no run":                          view.RoleRefusal,
		"· run finished, exit 0":                   view.RoleNote,
		"> /help":                                  view.RoleYou,
		helpLines[1]:                               view.RoleText,
		"· `/nonsense` is not a command here. /help lists them.": view.RoleNote,
	}
	seen := map[string]bool{}
	roles := map[view.Role]bool{}
	for _, r := range m.Transcript {
		roles[r.Role] = true
		w, ok := want[r.Text]
		if !ok {
			continue
		}
		seen[r.Text] = true
		if r.Role != w {
			t.Errorf("%q has role %d, want %d", r.Text, r.Role, w)
		}
	}
	for text := range want {
		if !seen[text] {
			t.Errorf("no row %q in:\n%s", text, strings.Join(texts(m), "\n"))
		}
	}
	for _, r := range []view.Role{view.RoleYou, view.RoleTool, view.RolePassed, view.RoleFailed,
		view.RoleOutput, view.RoleText, view.RoleNote, view.RoleRefusal} {
		if !roles[r] {
			t.Errorf("no row took role %d: the run above no longer writes every kind", r)
		}
	}
	// A turn's rows carry its turn; every other row carries none.
	for _, r := range m.Transcript {
		fromTurn := r.Role == view.RoleTool || r.Role == view.RolePassed || r.Role == view.RoleFailed || r.Role == view.RoleOutput
		if fromTurn != (r.Turn > 0) {
			t.Errorf("%q: role %d with turn %d", r.Text, r.Role, r.Turn)
		}
	}
}

// Every transcript row enters through rows.go: no other file in the
// package appends to the transcript, so none can add a row without a role.
func TestRowsOnlyRowsGoWritesTheTranscript(t *testing.T) {
	files, err := filepath.Glob("*.go")
	if err != nil {
		t.Fatal(err)
	}
	for _, f := range files {
		if f == "rows.go" || strings.HasSuffix(f, "_test.go") {
			continue
		}
		src, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		if strings.Contains(string(src), "Transcript = append(") {
			t.Errorf("%s appends to the transcript itself; write rows through push or pushRow (rows.go)", f)
		}
	}
}
