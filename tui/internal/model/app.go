package model

import (
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/rpc"
)

// Client is what App needs of the rpc child; *rpc.Client is one.
type Client interface {
	Send(map[string]any) error
	Quit() error
	Lines() <-chan rpc.Line
	Stderr() ([]string, int)
	Wait() error
}

// App is the Bubble Tea program: it feeds the child's lines and the keys to
// Model.Update and writes the model's outbox to the child, in order. All
// the IO is here; all the decisions are in Model.
type App struct {
	M *Model
	C Client
	// stderrSeen counts the stderr lines already given to the model.
	stderrSeen int
}

type (
	lineMsg       struct{ l rpc.Line }
	linesEndedMsg struct{}
)

// NewApp joins a model to a running child.
func NewApp(m *Model, c Client) *App { return &App{M: m, C: c} }

// Init sends what New queued, starts reading the child and starts the
// clock.
func (a *App) Init() tea.Cmd {
	a.flush()
	return tea.Batch(a.waitLine(), tick())
}

// tick is the program's clock: a TickMsg each second, for as long as the
// program runs.
func tick() tea.Cmd {
	return tea.Tick(time.Second, func(time.Time) tea.Msg { return TickMsg{} })
}

// Update applies one message to the model, then sends what it queued.
func (a *App) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var next tea.Cmd
	switch msg := msg.(type) {
	case lineMsg:
		a.M.Update(FrameMsg{Frame: frames.Parse(msg.l)})
		next = a.waitLine()
	case linesEndedMsg:
		// stdout closed: the child is exiting. Wait for it, never kill it.
		next = a.waitExit()
	case TickMsg:
		a.M.Update(msg)
		next = tick()
	default:
		a.M.Update(msg)
	}
	a.syncStderr()
	a.flush()
	if a.M.Done {
		return a, tea.Quit
	}
	return a, next
}

// View draws the model.
func (a *App) View() string { return a.M.View() }

func (a *App) waitLine() tea.Cmd {
	lines := a.C.Lines()
	return func() tea.Msg {
		l, ok := <-lines
		if !ok {
			return linesEndedMsg{}
		}
		return lineMsg{l}
	}
}

func (a *App) waitExit() tea.Cmd {
	return func() tea.Msg { return ExitedMsg{Err: a.C.Wait()} }
}

// flush writes the outbox in the order the model queued it: an abort
// before its quit, a set_model before the prompt it gates.
func (a *App) flush() {
	for _, o := range a.M.TakeOutbox() {
		var err error
		if o.Quit {
			err = a.C.Quit()
		} else {
			err = a.C.Send(o.Cmd)
		}
		if err != nil {
			a.M.Update(SendFailedMsg{Err: err})
		}
	}
}

// syncStderr gives the model the stderr lines it has not seen, as Human
// frames: text for a pane, never data.
func (a *App) syncStderr() {
	lines, lost := a.C.Stderr()
	total := lost + len(lines)
	fresh := total - a.stderrSeen
	if fresh <= 0 {
		return
	}
	if fresh > len(lines) {
		fresh = len(lines)
	}
	for _, l := range lines[len(lines)-fresh:] {
		a.M.Update(FrameMsg{Frame: frames.FromStderr(l)})
	}
	a.stderrSeen = total
}
