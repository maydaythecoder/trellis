package model

import (
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

// barOf is the status bar: the screen's last row since the shell (#851).
func barOf(m *Model) string {
	m.Update(tea.WindowSizeMsg{Width: 120, Height: 30})
	rows := strings.Split(m.View(), "\n")
	return rows[len(rows)-1]
}

// The bar says the model and the tier get_state reports, and the Trail
// above says TRELLIS; a set_model that succeeds asks get_state again,
// since the tier may now be known, and so does a run's end.
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
	if !strings.Contains(bar, "model ollama:qwen") || !strings.Contains(bar, "tier 1, native tool calls") {
		t.Fatalf("bar %q", bar)
	}
	if top := strings.SplitN(m.View(), "\n", 2)[0]; !strings.Contains(top, "TRELLIS") {
		t.Fatalf("the Trail does not name the program: %q", top)
	}
	submit(m, "t")
	m.TakeOutbox()
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	// A converged run's closing words are asked for too, for the summary
	// (#854).
	if got := kinds(m.TakeOutbox()); strings.Join(got, ",") != "get_state,get_last_assistant_text" {
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
	if bar := barOf(m); !strings.Contains(bar, "● clean") {
		t.Fatalf("%q", bar)
	}
	feed(m, turn(1, "edit a.py", map[string]any{"observation": "applied: true", "tool": "edit", "target": "a.py", "wrote": true}))
	feed(m, turn(2, "edit b.py", map[string]any{"observation": "applied: true", "tool": "edit", "target": "b.py", "wrote": true}))
	if bar := barOf(m); !strings.Contains(bar, "● 2 pending") {
		t.Fatalf("%q", bar)
	}
}
