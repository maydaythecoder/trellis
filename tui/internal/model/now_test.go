package model

import (
	"strings"
	"testing"
	"time"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
)

// stripOf is the Now strip's row as the screen draws it at 120x40.
func stripOf(t *testing.T, m *Model) string {
	t.Helper()
	m.Update(windowSize(120, 40))
	rows := strings.Split(m.View(), "\n")
	if len(rows) < 2 {
		t.Fatalf("no strip:\n%s", m.View())
	}
	return rows[1]
}

func modelCall(turn int) string {
	return jsonLine(map[string]any{"level": "INFO", "fields": map[string]any{"message": "model call", "turn": turn}})
}

// #850: the Now strip says what is happening now: the state word, the
// turn against the run's ceiling, and what the run is on, in the words the
// desktop uses; a question waiting on the person, and how to answer it; an
// ended run in run_end's own words and exit code, never "done" or
// "failed"; and IDLE before any run.
func TestNowStripShowsStateTurnAndActivity(t *testing.T) {
	m := fresh(t)
	if s := stripOf(t, m); !strings.Contains(s, "IDLE") || strings.Contains(s, "turn") {
		t.Fatalf("before a run: %q", s)
	}
	m = running(t)
	feed(m, jsonLine(map[string]any{"level": "INFO", "fields": map[string]any{"message": "run budget", "max_iterations": 40}}))
	feed(m, modelCall(1))
	if s := stripOf(t, m); !strings.Contains(s, "RUNNING") || !strings.Contains(s, "turn 1/40") || !strings.Contains(s, "calling the model, turn 1") {
		t.Fatalf("calling the model: %q", s)
	}
	feed(m, turn(1, "edit app/calc.py", map[string]any{"observation": "applied: true"}))
	if s := stripOf(t, m); !strings.Contains(s, "turn 1/40") || !strings.Contains(s, "ran edit, turn 1 → app/calc.py") {
		t.Fatalf("after the turn: %q", s)
	}
	feed(m, approval("a1", "edit app/calc.py", "+x"))
	if s := stripOf(t, m); !strings.Contains(s, "WAITING ON YOU") || !strings.Contains(s, "y or n answers it") {
		t.Fatalf("a question waiting: %q", s)
	}
	key(m, 'n')
	feed(m, runEnd(map[string]any{"outcome": "not_converged", "exit": 1, "restored": true}))
	s := stripOf(t, m)
	if !strings.Contains(s, "ENDED · not_converged · exit 1") {
		t.Fatalf("ended: %q", s)
	}
	for _, refused := range []string{"done", "failed", "RUNNING"} {
		if strings.Contains(s, refused) {
			t.Fatalf("the ended strip says %q: %q", refused, s)
		}
	}
}

// #850: Where it is names the runtime as it identified itself, the
// repository trellis serves, and the control mode as not reported, since
// trellis sends a client no control mode; before the runtime says what it
// is, it too is not reported.
func TestWhereItIsShowsTheRuntime(t *testing.T) {
	m := running(t)
	m.Update(windowSize(120, 40))
	if v := m.View(); !strings.Contains(v, "runtime  not reported") || !strings.Contains(v, "control  not reported") {
		t.Fatalf("before the runtime said what it is:\n%s", v)
	}
	feed(m, jsonLine(map[string]any{"level": "INFO", "fields": map[string]any{
		"message": "runtime identified", "runtime": "ollama", "served": "qwen2.5-coder:32b"}}))
	m.refreshState()
	var asked any
	for _, o := range m.TakeOutbox() {
		if o.Cmd != nil && o.Cmd["type"] == "get_state" {
			asked = o.Cmd["id"]
		}
	}
	respond(m, asked, "get_state", map[string]any{"model": "ollama:qwen2.5-coder:32b", "root": "/work/trellis-demo"})
	v := m.View()
	for _, want := range []string{"┌ where", "runtime  ollama · qwen2.5-coder:32b", "root     trellis-demo", "control  not reported"} {
		if !strings.Contains(v, want) {
			t.Fatalf("%q is not in Where it is:\n%s", want, v)
		}
	}
}

// #850: the messages row draws trellis's newest stderr line, with how
// many came before it, and is not drawn until trellis has written one.
func TestViewDrawsTheNewestStderrLine(t *testing.T) {
	m := fresh(t)
	m.Update(windowSize(100, 30))
	if strings.Contains(m.View(), "trellis: ") {
		t.Fatalf("a messages row with nothing in it:\n%s", m.View())
	}
	for _, l := range []string{"index: 41 modules", "probe: colibri not listening", "model: qwen ready"} {
		m.Update(FrameMsg{Frame: frames.FromStderr(l)})
	}
	v := m.View()
	if !strings.Contains(v, "trellis: model: qwen ready  (2 earlier)") || strings.Contains(v, "colibri not listening") {
		t.Fatalf("the newest line, and only it:\n%s", v)
	}
	if n := len(strings.Split(v, "\n")); n != 30 {
		t.Fatalf("%d rows with the messages row, want 30", n)
	}
}

// #850: trellis does not report its own elapsed time yet (#906), so the
// strip's clock is this view's and says so, and it leaves out the time a
// question waited on the person, as trellis's budget does: 10 s of run, 30 s
// waiting on a question, then 5 s more read 0:15, while the wait is on and
// after it.
func TestNowClockIsLabelledAndSkipsApprovalWaits(t *testing.T) {
	m := fresh(t)
	clock := time.Date(2026, 10, 26, 7, 0, 0, 0, time.UTC)
	m.now = func() time.Time { return clock }
	submit(m, "t")
	feed(m, jsonLine(map[string]any{"type": "run_start", "id": "tui-3"}))
	clock = clock.Add(10 * time.Second)
	feed(m, approval("a1", "edit a.py", "+x"))
	clock = clock.Add(30 * time.Second)
	m.Update(TickMsg{})
	if s := stripOf(t, m); !strings.Contains(s, "0:10 by this view's clock") {
		t.Fatalf("while the question waits the clock holds at 0:10: %q", s)
	}
	key(m, 'y')
	clock = clock.Add(5 * time.Second)
	m.Update(TickMsg{})
	if s := stripOf(t, m); !strings.Contains(s, "0:15 by this view's clock") {
		t.Fatalf("after the answer: %q", s)
	}
	if v := m.View(); !strings.Contains(v, "0:15 elapsed") {
		t.Fatalf("the budget line's figure is not the strip's:\n%s", v)
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	clock = clock.Add(20 * time.Second)
	m.Update(TickMsg{})
	if s := stripOf(t, m); !strings.Contains(s, "0:15 by this view's clock") || !strings.Contains(s, "ENDED · converged · exit 0") {
		t.Fatalf("an ended run keeps the time it took: %q", s)
	}
}
