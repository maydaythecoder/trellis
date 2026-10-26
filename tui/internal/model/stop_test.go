package model

import (
	"errors"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/rpc"
)

// #634: leaving mid-run used to leave the run editing the tree with nobody
// watching. Esc on an empty line now stops it as ^c does: abort, which
// rolls the run back, then quit, once, said on screen; Esc with text only
// clears the line; and the program ends only when the child has exited,
// after the rollback.
func TestModelStopMidRunEscOnAnEmptyLineStopsAsCtrlCDoes(t *testing.T) {
	m := running(t)
	typeLine(m, "half a steer")
	m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	if m.Input != "" || m.Quitting {
		t.Fatalf("esc with text: input %q quitting %v", m.Input, m.Quitting)
	}
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("esc with text sent %v", kinds(out))
	}
	m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	if got := kinds(m.TakeOutbox()); strings.Join(got, ",") != "abort,QUIT" {
		t.Fatalf("esc on an empty line mid-run sent %v, want abort then quit", got)
	}
	if last := texts(m)[len(m.Transcript)-1]; !strings.HasPrefix(last, "· stopping:") {
		t.Fatalf("the stop is not said: %q", last)
	}
	m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("a second press asked again: %v", kinds(out))
	}
	if m.Done {
		t.Fatal("done before the child exited; the rollback may still be running")
	}
	m.Update(ExitedMsg{})
	if !m.Done {
		t.Fatal("not done once the child exited")
	}
}

// A run that has ended has nothing to stop: ^c then only quits.
func TestModelStopMidRunNoLongerAppliesAfterTheRunEnded(t *testing.T) {
	m := running(t)
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	m.TakeOutbox()
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	if got := kinds(m.TakeOutbox()); strings.Join(got, ",") != "QUIT" {
		t.Fatalf("sent %v, want quit alone", got)
	}
	for _, l := range texts(m) {
		if strings.HasPrefix(l, "· stopping:") {
			t.Fatalf("a stop was said with no run: %q", l)
		}
	}
}

// ^c with a question up withdraws it without answering: an answer would
// reach the gate before the run took the stop path. A question that
// arrives after the stop is not shown, since nobody waits for its answer;
// trellis drops both once the run has stopped, which is a no (ADR 0018).
func TestModelStopMidRunWithdrawsTheQuestionAndShowsNoNewOne(t *testing.T) {
	m := running(t)
	feed(m, approval("a1", "edit a.py (+1 -1)", "- a\n+ b"))
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	if got := kinds(m.TakeOutbox()); strings.Join(got, ",") != "abort,QUIT" || m.Modal != nil {
		t.Fatalf("sent %v, modal %+v", got, m.Modal)
	}
	feed(m, approval("a2", "edit a.py (+1 -1)", "- c\n+ d"))
	if m.Modal != nil || len(m.approvals) != 0 {
		t.Fatalf("a question asked after the stop was shown: %+v", m.Modal)
	}
	key(m, 'y')
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("a key after the stop answered something: %v", kinds(out))
	}
}

// goneClient is a child that has already gone: every write fails and its
// stdout is closed.
type goneClient struct{ lines chan rpc.Line }

func (g *goneClient) Send(map[string]any) error { return errors.New("broken pipe") }
func (g *goneClient) Quit() error               { return errors.New("broken pipe") }
func (g *goneClient) Lines() <-chan rpc.Line    { return g.lines }
func (g *goneClient) Stderr() ([]string, int)   { return nil, 0 }
func (g *goneClient) Wait() error               { return errors.New("exit status 1") }

// A stop with nobody to carry it out does not hang the program (events.rs:
// ctrl_c_mid_run_with_nothing_to_stop_it_leaves_at_once). The Go TUI's
// only driver is its rpc child; when that has gone, the failed writes are
// said and the program ends on the child's exit, the path every ending
// takes, rather than waiting for a run_end nobody can send.
func TestModelStopWithNoChildToStopStillEnds(t *testing.T) {
	g := &goneClient{lines: make(chan rpc.Line)}
	close(g.lines)
	m := New(Options{})
	m.TakeOutbox()
	submit(m, "change f")
	m.TakeOutbox()
	a := NewApp(m, g)
	a.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	// The program's loop: run each command and feed back what it returns.
	cmd := a.waitLine()
	for i := 0; i < 5 && cmd != nil; i++ {
		msg := cmd()
		_, cmd = a.Update(msg)
		if a.M.Done {
			break
		}
	}
	if !a.M.Done {
		t.Fatal("the program did not end with no child to stop the run")
	}
	said := strings.Join(texts(a.M), "\n")
	if !strings.Contains(said, "could not write to trellis rpc") {
		t.Fatalf("the failed stop was not said:\n%s", said)
	}
}
