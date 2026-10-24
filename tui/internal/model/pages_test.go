package model

import (
	"fmt"
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/rpc"
)

// numbered is a model at 80x24 whose transcript is n numbered lines,
// following its end.
func numbered(t *testing.T, n int) *Model {
	t.Helper()
	m := fresh(t)
	m.Update(windowSize(80, 24))
	m.Transcript = nil
	for i := 0; i < n; i++ {
		m.pushLine(fmt.Sprintf("line %03d", i))
	}
	return m
}

// #660: the arrows move one row, and the start of a long run was hundreds
// of presses away. PgUp moves ten rows back.
func TestPanesPageUpMovesTenRowsBack(t *testing.T) {
	m := numbered(t, 200)
	m.Update(keyType(tea.KeyPgUp))
	if m.ScrollBack != 10 {
		t.Fatalf("scroll back %d, want 10", m.ScrollBack)
	}
	m.Update(keyType(tea.KeyPgUp))
	if m.ScrollBack != 20 || !strings.Contains(m.View(), "line 179") {
		t.Fatalf("scroll back %d; twenty rows above the end is not drawn:\n%s", m.ScrollBack, m.View())
	}
}

// PgDn moves ten rows forward and saturates at the end, following again.
func TestPanesPageDownMovesTenRowsForwardAndStopsAtTheEnd(t *testing.T) {
	m := numbered(t, 200)
	m.ScrollBack = 25
	m.Update(keyType(tea.KeyPgDown))
	if m.ScrollBack != 15 {
		t.Fatalf("scroll back %d, want 15", m.ScrollBack)
	}
	m.Update(keyType(tea.KeyPgDown))
	m.Update(keyType(tea.KeyPgDown))
	if m.ScrollBack != 0 {
		t.Fatalf("scroll back %d, want 0: saturating at the end", m.ScrollBack)
	}
}

// Home shows the first line, counted in wrapped rows so lines that wrap
// in the pane do not stop it short of the top, and PgUp from there stays.
func TestPanesHomeShowsTheFirstLineAndPageUpStopsThere(t *testing.T) {
	m := fresh(t)
	m.Update(windowSize(80, 24))
	m.Transcript = nil
	for i := 0; i < 120; i++ {
		m.pushLine(fmt.Sprintf("line %03d %s", i, strings.Repeat("wrapped words ", 5)))
	}
	if strings.Contains(m.View(), "line 000") {
		t.Fatal("it does not start at the end")
	}
	m.Update(keyType(tea.KeyHome))
	if !strings.Contains(m.View(), "line 000") {
		t.Fatalf("home does not show the first line:\n%s", m.View())
	}
	top := m.ScrollBack
	m.Update(keyType(tea.KeyPgUp))
	if m.ScrollBack != top {
		t.Fatalf("scroll back %d after PgUp at the top %d", m.ScrollBack, top)
	}
}

// End follows the end again: a line that arrives then is in view.
func TestPanesEndFollowsTheEndAgain(t *testing.T) {
	m := numbered(t, 200)
	m.Update(keyType(tea.KeyHome))
	if m.ScrollBack == 0 {
		t.Fatal("home did not scroll back")
	}
	m.Update(keyType(tea.KeyEnd))
	if m.ScrollBack != 0 {
		t.Fatalf("scroll back %d after End", m.ScrollBack)
	}
	m.pushLine("THE NEWEST")
	if !strings.Contains(m.View(), "THE NEWEST") {
		t.Fatalf("not following:\n%s", m.View())
	}
}

// #641: between turns nothing on screen moved, and a long model call read
// as a hang. The clock keeps elapsed current while a run is in flight; it
// says nothing before a second has been measured, and a tick after the
// run ended does not restart it.
func TestPanesClockKeepsElapsedCurrentWhileRunning(t *testing.T) {
	m := fresh(t)
	clock := time.Date(2026, 10, 12, 12, 0, 0, 0, time.UTC)
	m.now = func() time.Time { return clock }
	submit(m, "t")
	feed(m, jsonLine(map[string]any{"type": "run_start", "id": "tui-3"}))
	feed(m, jsonLine(map[string]any{"level": "INFO", "fields": map[string]any{"message": "run budget", "max_iterations": 40}}))
	m.Update(TickMsg{})
	if m.Budget.ElapsedSecs != nil {
		t.Fatalf("elapsed %d before a second was measured", *m.Budget.ElapsedSecs)
	}
	clock = clock.Add(75 * time.Second)
	m.Update(TickMsg{})
	if m.Budget.ElapsedSecs == nil || *m.Budget.ElapsedSecs != 75 {
		t.Fatalf("elapsed %v, want 75", m.Budget.ElapsedSecs)
	}
	m.Update(windowSize(120, 24))
	if v := m.View(); !strings.Contains(v, "1:15 elapsed") || !strings.Contains(v, "no iteration yet of 40") {
		t.Fatalf("view:\n%s", v)
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	clock = clock.Add(15 * time.Second)
	m.Update(TickMsg{})
	if *m.Budget.ElapsedSecs != 75 {
		t.Fatalf("a tick after the run ended moved elapsed to %d", *m.Budget.ElapsedSecs)
	}
}

// The program's clock (tui_run.rs's send_elapsed): a TickMsg after a
// measured second, and another scheduled each time one arrives.
func TestPanesClockTicksOnceASecond(t *testing.T) {
	start := time.Now()
	msg := tick()()
	if _, ok := msg.(TickMsg); !ok {
		t.Fatalf("got %T, want TickMsg", msg)
	}
	if d := time.Since(start); d < 900*time.Millisecond {
		t.Fatalf("ticked after %v, not a second", d)
	}
	a := NewApp(fresh(t), &fakeClient{lines: make(chan rpc.Line)})
	if _, next := a.Update(TickMsg{}); next == nil {
		t.Fatal("no next tick was scheduled")
	}
}

// A turn that did not report its prompt size is not a 0-byte prompt: the
// figure stays unreported, and the line says so.
func TestPanesTurnWithoutAPromptSizeLeavesItUnreported(t *testing.T) {
	m := running(t)
	feed(m, turn(3, "read_file a.py", map[string]any{"observation": "..."}))
	if m.Budget.PromptBytes != nil || m.Budget.ElapsedSecs == nil {
		t.Fatalf("budget %+v", m.Budget)
	}
	m.Update(windowSize(120, 24))
	if v := m.View(); !strings.Contains(v, "prompt not reported") || strings.Contains(v, "0 B prompt") {
		t.Fatalf("view:\n%s", v)
	}
}
