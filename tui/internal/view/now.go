package view

import (
	"fmt"
	"strings"
)

// The Now strip, Where it is and the messages row (#850): the one row under
// the status bar that says what is happening right now, the pane that says
// where the run is, and trellis's newest stderr line. Every figure is one
// trellis reported or this view's own clock, labelled as its own; one that
// was not reported is said to be, never drawn as 0 or guessed.

// Phase is what a run is on, as its last event said: the model being
// called for a turn, or the tool the last turn ran.
type Phase struct {
	// Kind is "model" while the model is called, "tool" once a turn has
	// run a tool, and "" before the first call and once the run has ended.
	Kind string
	// Turn is the turn trellis numbered the event with.
	Turn uint64
	// Tool and Target are the last turn's call, split as the panes split
	// it; empty for a model call.
	Tool, Target string
}

// Now is what the Now strip and Where it is draw that the rest of State
// does not hold.
type Now struct {
	Phase Phase
	// Waiting is set while a question trellis asked waits on the person.
	Waiting bool
	// Stopping is set once a stop was sent and the run has not ended.
	Stopping bool
	// Outcome and Exit are the last run_end's own words, for an ended run;
	// "" and nil when no run has ended or it did not say.
	Outcome string
	Exit    *int64
	// Runtime is what the runtime identified itself as, "" until it has.
	Runtime string
	// Root is the repository trellis serves, from get_state; "" until it
	// has said.
	Root string
}

// whereH is the Where it is pane's height: phase, runtime, control and
// root, and two borders.
const whereH = 6

// stateWord is the run's state as the strip names it: run_end's own words
// for an ended run, never "done" or "failed" (DESIGN.md §2.1).
func stateWord(s State) string {
	switch {
	case s.Now.Stopping && s.Running:
		return "STOPPING"
	case s.Running && s.Now.Waiting:
		return "WAITING ON YOU"
	case s.Running:
		return "RUNNING"
	case s.Now.Outcome != "":
		word := "ENDED · " + Plain(s.Now.Outcome)
		if s.Now.Exit != nil {
			word += fmt.Sprintf(" · exit %d", *s.Now.Exit)
		}
		return word
	}
	return "IDLE"
}

// Activity is what the run is on, in words, as the desktop's status bar
// says it: "calling the model, turn 3", "ran edit, turn 3".
func Activity(p Phase) string {
	turn := ""
	if p.Turn > 0 {
		turn = fmt.Sprintf(", turn %d", p.Turn)
	}
	switch p.Kind {
	case "model":
		return "calling the model" + turn
	case "tool":
		return "ran " + Plain(p.Tool) + turn
	}
	return "none"
}

// nowStrip is the Now strip's one row: the state word, the elapsed time
// with whose clock it is, the turn against its ceiling, and what the run
// is on, the last turn's tool → target when it ran one.
func nowStrip(s State, w int) string {
	parts := []string{" " + stateWord(s)}
	if s.Running || s.Now.Outcome != "" {
		parts = append(parts, clockText(s))
		parts = append(parts, turnText(s))
	}
	// Before its first model call a run is on nothing yet, and the strip
	// says nothing for it rather than "none".
	if p := s.Now.Phase; s.Running && p.Kind != "" {
		on := Activity(p)
		if p.Kind == "tool" && p.Target != "" {
			on += " → " + Plain(p.Target)
		}
		parts = append(parts, on)
	}
	if s.Running && s.Now.Waiting {
		parts = append(parts, "y or n answers it")
	}
	return Cut(strings.Join(parts, "  "), w)
}

// clockText is the elapsed time and whose clock measured it. trellis does
// not report its own yet (#906), so it is this view's, paused while a
// question waited on the person.
func clockText(s State) string {
	if s.Budget.ElapsedSecs == nil {
		return "0:00 by this view's clock"
	}
	return Clock(*s.Budget.ElapsedSecs) + " by this view's clock"
}

// turnText is the turn against the run's own ceiling: the turn the model
// is being called for, or the last one reported.
func turnText(s State) string {
	n := s.Budget.Iteration
	if s.Now.Phase.Kind == "model" && s.Now.Phase.Turn > n {
		n = s.Now.Phase.Turn
	}
	switch {
	case n == 0 && s.Budget.MaxIterations == nil:
		return "no turn yet"
	case n == 0:
		return fmt.Sprintf("no turn yet of %d", *s.Budget.MaxIterations)
	case s.Budget.MaxIterations == nil:
		return fmt.Sprintf("turn %d", n)
	}
	return fmt.Sprintf("turn %d/%d", n, *s.Budget.MaxIterations)
}

// where is the Where it is pane: the run's phase, the runtime, the control
// mode and the repository. trellis reports no control mode to a client
// yet, so it is said to be unreported.
func where(s State, w, h int) []string {
	runtime := Plain(s.Now.Runtime)
	if runtime == "" {
		runtime = "not reported"
	}
	root := "not reported"
	if r := Plain(s.Now.Root); r != "" {
		root = baseName(r)
	}
	rows := []Row{
		{Text: "  phase    " + Activity(s.Now.Phase)},
		{Text: "  runtime  " + runtime},
		{Text: "  control  not reported", Role: RoleLabel},
		{Text: "  root     " + root},
	}
	return Box(s.Theme, "where", "", rows, w, h)
}

// messages is the messages row: trellis's newest stderr line, human text
// it wrote for a person (ADR 0052 section 4), with how many came before.
// It is drawn only once trellis has written one.
func messages(s State, w int) string {
	if len(s.Stderr) == 0 {
		return ""
	}
	line := " trellis: " + Plain(s.Stderr[len(s.Stderr)-1])
	if n := len(s.Stderr) - 1; n > 0 {
		line += fmt.Sprintf("  (%d earlier)", n)
	}
	return fill(s.Theme, RoleOutput, line, w)
}

// baseName is a path's last component, split on either separator, since
// trellis may run on Windows; a path that is all separators is itself.
func baseName(path string) string {
	parts := strings.FieldsFunc(path, func(r rune) bool { return r == '/' || r == '\\' })
	if len(parts) == 0 {
		return path
	}
	return parts[len(parts)-1]
}

// askingRows is the rows drawn above the approval box: the status bar and
// the Now strip, which stay on screen while a question waits, so the
// person answering sees the run's state and how to answer (#850).
const askingRows = 2

// Asking is the screen while a question waits on the person: the status
// bar and the Now strip, then the approval box (Modal) over the rest,
// which covers the panes, so the keys on screen are the ones that act.
func Asking(s State, question, diff string, scroll, w, h int) string {
	if w < MinWidth || h < MinHeight {
		return tooSmall(w, h)
	}
	top := fill(s.Theme, RoleLabel, status(s, w), w) + "\n" + fill(s.Theme, nowRole(s), nowStrip(s, w), w)
	return top + "\n" + Modal(s.Theme, question, diff, scroll, w, h-askingRows)
}

// AskingLastFrom is ModalLastFrom for the box Asking draws at w by h: the
// furthest line the model scrolls the question's diff to.
func AskingLastFrom(diff string, w, h int) int {
	return ModalLastFrom(diff, w, max(h-askingRows, 1))
}
