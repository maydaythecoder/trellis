package model

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func steerDelivered(afterStep int) string {
	return jsonLine(map[string]any{"level": "INFO", "fields": map[string]any{"message": "steer delivered", "after_step": afterStep}})
}

func last(m *Model) string { return m.Transcript[len(m.Transcript)-1] }

// A line typed mid-run is queued as `steer {message}` for that run, shown
// as queued, and never sent as a second prompt.
func TestSteerMidRunLineIsASteerNotATask(t *testing.T) {
	m := running(t)
	typeLine(m, "use the helper")
	enter(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "steer" || out[0].Cmd["message"] != "use the helper" {
		t.Fatalf("sent %+v", out)
	}
	if last(m) != "· queued: use the helper" || m.Input != "" {
		t.Fatalf("last %q input %q", last(m), m.Input)
	}
}

// A /command mid-run is refused with the Rust TUI's wording, nothing is
// sent, and the line is kept.
func TestSteerSlashCommandMidRunIsRefused(t *testing.T) {
	m := running(t)
	typeLine(m, "/help")
	enter(m)
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("sent %+v", out)
	}
	if last(m) != "· a run is already in flight; wait for it to finish" || m.Input != "/help" {
		t.Fatalf("last %q input %q", last(m), m.Input)
	}
}

// The steer delivered event marks the steers in the order they were sent,
// each "delivered at turn N" with N = after_step + 1, once: trellis's
// answer to the same steer, arriving after, writes nothing again.
func TestSteerDeliveredOnceAtItsTurn(t *testing.T) {
	m := running(t)
	typeLine(m, "first")
	enter(m)
	typeLine(m, "second")
	enter(m)
	ids := m.TakeOutbox()
	feed(m, steerDelivered(2))
	feed(m, steerDelivered(4))
	n := len(m.Transcript)
	feed(m, jsonLine(map[string]any{"type": "response", "command": "steer", "id": ids[0].Cmd["id"], "success": true, "data": map[string]any{"after_step": 2}}))
	feed(m, steerDelivered(4))
	got := m.Transcript[len(m.Transcript)-2:]
	if len(m.Transcript) != n || got[0] != "· delivered at turn 3: first" || got[1] != "· delivered at turn 5: second" {
		t.Fatalf("transcript tail %q (len %d, want %d)", m.Transcript[len(m.Transcript)-3:], len(m.Transcript), n)
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if strings.Contains(strings.Join(m.Transcript, "\n"), "not delivered") {
		t.Fatalf("a delivered steer was called not delivered:\n%s", strings.Join(m.Transcript, "\n"))
	}
}

// trellis's answer alone also marks a steer delivered, before the run
// ends; a steer the run ended before, refused by trellis, or handed back
// by abort, is said to be not delivered, once.
func TestSteerAnswersMarkItDeliveredOrNot(t *testing.T) {
	m := running(t)
	for _, s := range []string{"a", "b", "c", "d"} {
		typeLine(m, s)
		enter(m)
	}
	out := m.TakeOutbox()
	feed(m, jsonLine(map[string]any{"type": "response", "command": "steer", "id": out[0].Cmd["id"], "success": true, "data": map[string]any{"after_step": 0}}))
	feed(m, jsonLine(map[string]any{"type": "response", "command": "steer", "id": out[1].Cmd["id"], "success": false, "error": "the run ended before its next model request; not delivered"}))
	feed(m, jsonLine(map[string]any{"type": "response", "command": "abort", "id": "x", "success": true,
		"data": map[string]any{"undelivered": []any{map[string]any{"id": out[2].Cmd["id"], "type": "steer", "message": "c"}}}}))
	feed(m, runEnd(map[string]any{"outcome": "interrupted", "exit": 130, "restored": true}))
	tail := strings.Join(m.Transcript[len(m.Transcript)-5:], "\n")
	want := strings.Join([]string{
		"· delivered at turn 1: a",
		"· not delivered: b",
		"· not delivered: c",
		"· not delivered: d",
		"· run finished, exit 130",
	}, "\n")
	if tail != want {
		t.Fatalf("got:\n%s\nwant:\n%s", tail, want)
	}
}

// Steers are counted per run: a delivery event in the next run names that
// run's first steer, not one left over from the last.
func TestSteerCountedPerRun(t *testing.T) {
	m := running(t)
	typeLine(m, "old")
	enter(m)
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	typeLine(m, "next task")
	enter(m)
	typeLine(m, "new")
	enter(m)
	feed(m, steerDelivered(0))
	if last(m) != "· delivered at turn 1: new" {
		t.Fatalf("last %q", last(m))
	}
}

// A line typed after its run ended is a task, not a steer for the run
// that has gone.
func TestSteerAfterTheRunEndedIsATask(t *testing.T) {
	m := running(t)
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	m.TakeOutbox() // the get_state a run's end asks
	typeLine(m, "another")
	enter(m)
	if got := kinds(m.TakeOutbox()); strings.Join(got, ",") != "prompt" {
		t.Fatalf("sent %v", got)
	}
}

// A steer is text the model was shown and nothing more: delivered, it adds
// no check line and does not end the run (ADR 0004).
func TestSteerIsNeverAVerdict(t *testing.T) {
	m := running(t)
	typeLine(m, "tests pass now")
	enter(m)
	feed(m, steerDelivered(0))
	for _, l := range m.Transcript {
		if strings.Contains(l, "check passed") || strings.Contains(l, "FAILED") {
			t.Fatalf("a steer became a verdict: %q", l)
		}
	}
	if !m.Running || m.Outcome != "" {
		t.Fatal("a steer ended the run")
	}
}

// While a run is in flight the input line shows the steer being typed,
// and says that Enter queues it.
func TestSteerLineIsShownWhileRunning(t *testing.T) {
	m := running(t)
	typeLine(m, "half typed")
	v := m.View()
	if !strings.Contains(v, "half typed█") || !strings.Contains(v, "running — enter queues a steer") {
		t.Fatalf("view:\n%s", v)
	}
}

// Esc clears a line and leaves an empty one; on an empty line with no run
// in flight it quits (mid-run, TestModelStopMidRun).
func TestModelEscClearsTheLineThenQuitsWhenIdle(t *testing.T) {
	m := running(t)
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	m.TakeOutbox()
	typeLine(m, "abc")
	m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	if m.Input != "" || m.Quitting {
		t.Fatalf("input %q quitting %v", m.Input, m.Quitting)
	}
	m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	if got := kinds(m.TakeOutbox()); !m.Quitting || strings.Join(got, ",") != "QUIT" {
		t.Fatalf("quitting %v sent %v", m.Quitting, got)
	}
}
