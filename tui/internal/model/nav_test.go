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

// #854: the summary is a level like a pane's, reached mid-run as "so far":
// Tab selects it after the panes, ⏎ goes in, the arrows scroll it within
// its own rows, and ESC comes back out to the top with it still selected.
// It draws what the model holds, including every ceiling run budget sent
// and the steers once the run has ended, which the run's end used to
// forget; going in sends nothing.
func TestSummaryReachedMidRunScrollsAndKeepsTheRunsSteers(t *testing.T) {
	m := running(t)
	feed(m, jsonLine(map[string]any{"level": "INFO", "fields": map[string]any{"message": "run budget",
		"max_iterations": 9, "max_wall_clock_ms": 321000, "max_completion_tokens": 54321, "max_context_bytes": 123456}}))
	feed(m, turn(1, "run_checks", map[string]any{"verdict": "Some(true)", "check_kind": "Typecheck",
		"duration_ms": 120, "completion_tokens_used": 77, "prompt_bytes": 2000}))
	typeLine(m, "use the helper")
	enter(m)
	m.TakeOutbox()
	for range view.Panels {
		if m.nav.Selected == view.PanelSummary {
			break
		}
		tabKey(m)
	}
	enter(m)
	m.Update(windowSize(80, 24))
	out := m.View()
	if m.nav.Open != view.PanelSummary || !strings.HasPrefix(out, " Run ⟩ summary") || !strings.Contains(out, "so far") {
		t.Fatalf("⏎ on the selected summary did not go in:\n%s", out)
	}
	for range 200 {
		m.Update(keyType(tea.KeyDown))
	}
	w, h := m.size()
	if top := view.SummaryTop(m.viewState(), w, h); m.nav.Scroll != top || top == 0 {
		t.Fatalf("scrolled to %d, the level's last page is at %d", m.nav.Scroll, top)
	}
	for _, want := range []string{"1/9", "5:21", "77/54321 tokens", "2.0 kB/123.5 kB", "queued · steer: use the helper"} {
		if !strings.Contains(m.summaryText(), want) {
			t.Fatalf("the summary does not hold %q:\n%s", want, m.summaryText())
		}
	}
	feed(m, runEnd(map[string]any{"outcome": "not_converged", "exit": 1, "restored": true}))
	if s := m.summaryText(); !strings.Contains(s, "not delivered (the run ended first) · steer: use the helper") || !strings.Contains(s, "120 ms") {
		t.Fatalf("the ended run's steer or check is gone from its summary:\n%s", s)
	}
	escKey(m)
	if m.nav.Open != "" || m.nav.Selected != view.PanelSummary || m.nav.Scroll != 0 {
		t.Fatalf("ESC: open %q selected %q scroll %d", m.nav.Open, m.nav.Selected, m.nav.Scroll)
	}
	if out := m.TakeOutbox(); strings.Join(kinds(out), ",") != "get_state" {
		t.Fatalf("the summary sent %v; only run_end's get_state was expected", kinds(out))
	}
}

// summaryText is the summary level at 120 columns and tall enough that
// none of it scrolls out, from the top.
func (m *Model) summaryText() string {
	s := m.viewState()
	s.Nav = view.Nav{Open: view.PanelSummary}
	return view.Render(s, 120, 200)
}
