package model

import (
	"io"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/leaktest"
	"github.com/maydaythecoder/trellis/tui/internal/rpc"
)

// runProgram runs one program over a fake child whose stdout never closes,
// and quits it from outside as a key or a signal would, so Run returns with
// the line reader still waiting; then it closes the App, as main does.
func runProgram(t *testing.T) {
	t.Helper()
	f := &fakeClient{lines: make(chan rpc.Line)}
	app := NewApp(New(Options{}), f)
	p := tea.NewProgram(app, tea.WithInput(nil), tea.WithOutput(io.Discard), tea.WithoutSignals())
	ran := make(chan error, 1)
	go func() {
		_, err := p.Run()
		ran <- err
	}()
	// A line from the child, taken only once the program is reading: the
	// quit below then lands with the read of the next line already
	// waiting, as it does when a person quits mid-run. Quit any sooner and
	// Run could return before its commands started, leaving nothing to
	// find.
	f.lines <- rpc.Line{Text: "trellis: hello"}
	p.Quit()
	if err := <-ran; err != nil {
		t.Fatalf("program: %v", err)
	}
	app.Close()
}

// #847: a program that quit on a key or a signal left its line reader
// blocked on the child's stdout, and its clock running for up to a second
// more. Ten programs, run and quit, leave none of theirs behind. One runs
// first, before the snapshot, so that whatever the first program starts
// once for the whole process is not counted as a leak of the ten.
func TestAppProgramsLeaveNoGoroutineBehind(t *testing.T) {
	runProgram(t)
	defer leaktest.Check(t)()
	for i := 0; i < 10; i++ {
		runProgram(t)
	}
}

// Close is safe to call twice: main calls it after Run, and a later exit
// path may call it again.
func TestAppCloseTwiceDoesNothing(t *testing.T) {
	app := NewApp(New(Options{}), &fakeClient{lines: make(chan rpc.Line)})
	app.Close()
	app.Close()
}
