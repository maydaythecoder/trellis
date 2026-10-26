package model

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

func tabKey(m *Model)      { m.Update(tea.KeyMsg{Type: tea.KeyTab}) }
func shiftTabKey(m *Model) { m.Update(tea.KeyMsg{Type: tea.KeyShiftTab}) }
func escKey(m *Model)      { m.Update(tea.KeyMsg{Type: tea.KeyEsc}) }

// trailOf is the Trail row at 120x40.
func trailOf(m *Model) string {
	m.Update(windowSize(120, 40))
	return strings.SplitN(m.View(), "\n", 2)[0]
}

// #851: Tab selects a pane at the top level and ⏎ on an empty task line
// goes into it, a level down; ESC comes back out with the pane still
// selected, and ESC again lets go of the selection without quitting. Tab
// inside a pane goes across to the next one. A line with text is still a
// task on ⏎, and an @ word still takes Tab for its completions. Going in
// or out sends nothing.
func TestNavEnterInEscOut(t *testing.T) {
	m := fresh(t)
	tabKey(m)
	tabKey(m)
	if m.nav.Selected != view.PanelActions {
		t.Fatalf("two Tabs selected %q, want %q", m.nav.Selected, view.PanelActions)
	}
	enter(m)
	if m.nav.Open != view.PanelActions {
		t.Fatalf("⏎ on an empty line did not go in: open %q", m.nav.Open)
	}
	if top := trailOf(m); !strings.HasPrefix(top, " Run ⟩ actions") || !strings.Contains(top, "L2 / 4") {
		t.Fatalf("the Trail inside the pane reads %q", top)
	}
	tabKey(m)
	if m.nav.Open != view.PanelEdits {
		t.Fatalf("Tab inside a pane went to %q, want %q", m.nav.Open, view.PanelEdits)
	}
	shiftTabKey(m)
	escKey(m)
	if m.nav.Open != "" || m.nav.Selected != view.PanelActions {
		t.Fatalf("ESC: open %q selected %q", m.nav.Open, m.nav.Selected)
	}
	if top := trailOf(m); !strings.HasPrefix(top, " Run ") || !strings.Contains(top, "L1 / 4") {
		t.Fatalf("the Trail after ESC reads %q", top)
	}
	escKey(m)
	if m.nav.Selected != "" || m.Quitting {
		t.Fatalf("ESC at the top: selected %q quitting %v", m.nav.Selected, m.Quitting)
	}
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("moving between levels sent %v", kinds(out))
	}

	tabKey(m)
	typeLine(m, "fix it")
	enter(m)
	if m.nav.Open != "" {
		t.Fatal("⏎ with text went into a pane instead of sending the line")
	}
	if out := m.TakeOutbox(); len(out) != 1 || out[0].Cmd["type"] != "classify" {
		t.Fatalf("⏎ with text sent %v, want the line classified", kinds(out))
	}

	m = fresh(t)
	typeLine(m, "fix @mo")
	tabKey(m)
	if m.nav.Selected != "" {
		t.Fatalf("Tab after an @ word selected %q", m.nav.Selected)
	}
	if out := m.TakeOutbox(); len(out) != 1 || out[0].Cmd["type"] != "list_paths" {
		t.Fatalf("Tab after an @ word sent %v, want list_paths", kinds(out))
	}
}
