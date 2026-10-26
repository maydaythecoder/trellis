package model

import (
	"fmt"
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

// #853: inside the edits pane, the code level, ↑ and ↓ move the cursor
// over the files and ⏎ on an empty line goes one level down, to the
// cursor's file's diff (L3), where ↑ and ↓ scroll; ESC comes back to the
// code level with the cursor where it was, and again to the top. A long
// diff scrolls no further than its last page, so the first ↑ after the
// end moves (#843). Going in, out and scrolling send nothing.
func TestNavCodeLevelCursorAndDiff(t *testing.T) {
	m := running(t)
	m.Update(windowSize(120, 40))
	long := "--- b.py\n+++ b.py\n@@ -1,0 +1,80 @@\n"
	for i := range 80 {
		long += fmt.Sprintf("+line %d\n", i)
	}
	feed(m, turn(1, "edit a.py", map[string]any{"tool": "edit", "target": "a.py", "wrote": true,
		"lines_added": 1, "lines_removed": 0, "diff": "--- a.py\n+++ a.py\n@@ -1,0 +1,1 @@\n+x = 1\n"}))
	feed(m, turn(2, "edit b.py", map[string]any{"tool": "edit", "target": "b.py", "wrote": true,
		"lines_added": 80, "lines_removed": 0, "diff": long}))
	m.nav.Selected = view.PanelEdits
	enter(m)
	m.Update(tea.KeyMsg{Type: tea.KeyDown})
	m.Update(tea.KeyMsg{Type: tea.KeyDown})
	if m.nav.File != "b.py" {
		t.Fatalf("↓ twice put the cursor on %q, want the last file, b.py", m.nav.File)
	}
	enter(m)
	if v := m.View(); !m.nav.Diff || !strings.HasPrefix(v, " Run ⟩ edits ⟩ b.py") || !strings.Contains(v, "L3 / 4") || !strings.Contains(v, "line 0") {
		t.Fatalf("⏎ did not show b.py's diff:\n%s", v)
	}
	m.Update(tea.KeyMsg{Type: tea.KeyEnd})
	end := m.nav.Scroll
	if v := m.View(); end == 0 || !strings.Contains(v, "line 79") || strings.Contains(v, "line 0 ") {
		t.Fatalf("End did not show the diff's last page (scroll %d):\n%s", end, v)
	}
	m.Update(tea.KeyMsg{Type: tea.KeyDown})
	m.Update(tea.KeyMsg{Type: tea.KeyUp})
	if m.nav.Scroll != end-1 {
		t.Fatalf("↑ after the end moved to %d, want %d", m.nav.Scroll, end-1)
	}
	escKey(m)
	if m.nav.Diff || m.nav.Open != view.PanelEdits || m.nav.File != "b.py" {
		t.Fatalf("ESC from the diff: %+v", m.nav)
	}
	escKey(m)
	if m.nav.Open != "" {
		t.Fatalf("ESC from the code level: %+v", m.nav)
	}
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("moving through the code level sent %v", kinds(out))
	}
}
