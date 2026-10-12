package model

import (
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func respond(m *Model, id any, command string, data any) {
	feed(m, jsonLine(map[string]any{"type": "response", "command": command, "id": id, "success": true, "data": data}))
}

// startup is a model whose start-up get_state and set_model ids are known.
func startup(t *testing.T) (*Model, any, any) {
	t.Helper()
	m := New(Options{})
	out := m.TakeOutbox()
	return m, out[0].Cmd["id"], out[1].Cmd["id"]
}

func barOf(m *Model) string {
	m.Update(tea.WindowSizeMsg{Width: 120, Height: 30})
	return strings.SplitN(m.View(), "\n", 2)[0]
}

// The bar says TRELLIS, the model and the tier get_state reports; a
// set_model that succeeds asks get_state again, since the tier may now be
// known, and so does a run's end.
func TestStatusShowsModelAndTierFromGetState(t *testing.T) {
	m, stateID, modelID := startup(t)
	respond(m, stateID, "get_state", map[string]any{"model": nil, "tier": nil, "running": false})
	respond(m, modelID, "set_model", map[string]any{"model": "ollama:qwen", "configured": true, "saved_to": nil})
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "get_state" {
		t.Fatalf("after set_model: %+v", out)
	}
	respond(m, out[0].Cmd["id"], "get_state", map[string]any{"model": "ollama:qwen", "tier": "1, native tool calls"})
	bar := barOf(m)
	if !strings.Contains(bar, "TRELLIS") || !strings.Contains(bar, "model ollama:qwen") || !strings.Contains(bar, "tier 1, native tool calls") {
		t.Fatalf("bar %q", bar)
	}
	typeLine(m, "t")
	enter(m)
	m.TakeOutbox()
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if got := kinds(m.TakeOutbox()); strings.Join(got, ",") != "get_state" {
		t.Fatalf("after run_end: %v", got)
	}
}

// Until get_state has a tier the bar says probing…, also when a runtime
// that cannot be reached leaves it null: the client claims no tier.
func TestStatusProbingUntilATier(t *testing.T) {
	m, stateID, _ := startup(t)
	if bar := barOf(m); !strings.Contains(bar, "tier probing…") || !strings.Contains(bar, "model none") {
		t.Fatalf("before get_state: %q", bar)
	}
	respond(m, stateID, "get_state", map[string]any{"model": "llamacpp:http://127.0.0.1:1", "tier": nil})
	if bar := barOf(m); !strings.Contains(bar, "tier probing…") {
		t.Fatalf("a null tier: %q", bar)
	}
}

// The bar writes the word tier once before the label, and served text in
// it is plain.
func TestStatusTierWordOnceAndPlain(t *testing.T) {
	m, stateID, _ := startup(t)
	respond(m, stateID, "get_state", map[string]any{"model": "m\x1b[31m", "tier": "tier 2, tools probed"})
	bar := barOf(m)
	if strings.Contains(bar, "tier tier") || !strings.Contains(bar, "tier 2, tools probed") || strings.Contains(bar, "\x1b") {
		t.Fatalf("bar %q", bar)
	}
}

// The bar says clean, or how many edits are pending, from the edits pane.
func TestStatusWorktreeCleanOrEditsPending(t *testing.T) {
	m := running(t)
	if bar := barOf(m); !strings.Contains(bar, "worktree clean") {
		t.Fatalf("%q", bar)
	}
	feed(m, turn(1, "edit a.py", map[string]any{"observation": "applied: true"}))
	feed(m, turn(2, "edit b.py", map[string]any{"observation": "applied: true"}))
	if bar := barOf(m); !strings.Contains(bar, "worktree 2 edit(s) pending") {
		t.Fatalf("%q", bar)
	}
}

func tab(m *Model) { m.Update(tea.KeyMsg{Type: tea.KeyTab}) }

// Tab after @prefix sends list_paths {prefix}; one path completes the word
// with a space, several are listed in the order trellis returned them, and
// a second Tab inserts the first.
func TestCompleteOneChoiceCompletesSeveralAreListed(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix @src/mo")
	tab(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "list_paths" || out[0].Cmd["prefix"] != "src/mo" {
		t.Fatalf("sent %+v", out)
	}
	respond(m, out[0].Cmd["id"], "list_paths", map[string]any{"paths": []any{"src/models.py"}})
	if m.Input != "fix src/models.py " {
		t.Fatalf("input %q", m.Input)
	}
	typeLine(m, "@app")
	tab(m)
	out = m.TakeOutbox()
	respond(m, out[0].Cmd["id"], "list_paths", map[string]any{"paths": []any{"app/b.py", "app/a.py", "app/core/c.py"}})
	if strings.Join(m.Offered, ",") != "app/b.py,app/a.py,app/core/c.py" || m.Input != "fix src/models.py @app" {
		t.Fatalf("offered %q input %q", m.Offered, m.Input)
	}
	tab(m)
	if m.Input != "fix src/models.py app/b.py " || len(m.Offered) != 0 || len(m.TakeOutbox()) != 0 {
		t.Fatalf("second tab: input %q offered %q", m.Input, m.Offered)
	}
}

// Nothing offered changes nothing; an answer for a fragment no longer
// typed is dropped; only an @word at the very end is completed; a run in
// flight is not asked for paths.
func TestCompleteNothingOfferedOrStaleChangesNothing(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix @zz")
	tab(m)
	id := m.TakeOutbox()[0].Cmd["id"]
	respond(m, id, "list_paths", map[string]any{"paths": []any{}})
	if m.Input != "fix @zz" || len(m.Offered) != 0 {
		t.Fatalf("input %q offered %q", m.Input, m.Offered)
	}
	tab(m)
	id = m.TakeOutbox()[0].Cmd["id"]
	typeLine(m, "q")
	respond(m, id, "list_paths", map[string]any{"paths": []any{"zz.py"}})
	if m.Input != "fix @zzq" {
		t.Fatalf("a stale answer completed: %q", m.Input)
	}
	for _, line := range []string{"fix @a.py ", "fix a", "@"[:0]} {
		m.Input = line
		tab(m)
		if out := m.TakeOutbox(); len(out) != 0 {
			t.Fatalf("%q asked for paths: %+v", line, out)
		}
	}
	r := running(t)
	typeLine(r, "@src")
	tab(r)
	if out := r.TakeOutbox(); len(out) != 0 {
		t.Fatalf("a run in flight was asked for paths: %+v", out)
	}
}

// Candidates are drawn under the input line, the first marked as the one
// Tab inserts, and are not drawn while a run holds the input.
func TestCompleteCandidatesAreDrawnUnderTheInput(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix @models")
	tab(m)
	respond(m, m.TakeOutbox()[0].Cmd["id"], "list_paths", map[string]any{"paths": []any{"app/models.py", "src/models.py"}})
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	rows := strings.Split(m.View(), "\n")
	at := func(s string) int {
		for i, r := range rows {
			if strings.Contains(r, s) {
				return i
			}
		}
		t.Fatalf("%q not on screen:\n%s", s, strings.Join(rows, "\n"))
		return -1
	}
	typed, first, second := at("fix @models"), at("app/models.py"), at("src/models.py")
	if !(typed < first && first < second) || !strings.Contains(rows[first], "tab") || at("budget") < 0 || len(rows) != 24 {
		t.Fatalf("%s", strings.Join(rows, "\n"))
	}
}

// Enter with an @word sends the line as typed.
func TestCompleteEnterSubmitsTheLineAsTyped(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix @src/mo")
	enter(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[len(out)-1].Cmd["task"] != "fix @src/mo" {
		t.Fatalf("%+v", out)
	}
}

// The client never builds an exec.Cmd: no file of this package imports
// os/exec, so completion can only come from trellis (ADR 0023).
func TestCompleteNeverBuildsAnExecCmd(t *testing.T) {
	files, err := filepath.Glob("*.go")
	if err != nil || len(files) == 0 {
		t.Fatalf("no sources: %v", err)
	}
	// Built at run time, so this test's own text is not the import
	// go-guards.sh refuses outside internal/rpc.
	banned := map[string]bool{`"os/` + `exec"`: true, `"sys` + `call"`: true}
	fset := token.NewFileSet()
	for _, f := range files {
		src, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		af, err := parser.ParseFile(fset, f, src, parser.ImportsOnly)
		if err != nil {
			t.Fatal(err)
		}
		for _, imp := range af.Imports {
			if banned[imp.Path.Value] {
				t.Fatalf("%s imports %s", f, imp.Path.Value)
			}
		}
		if strings.Contains(string(src), "exec.Command") && !strings.HasSuffix(f, "_test.go") {
			t.Fatalf("%s names exec.Command", f)
		}
	}
}
