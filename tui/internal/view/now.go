package view

import (
	"fmt"
	"strings"
)

// The Now strip, Where it is and the messages row (#850): the one row under
// the Trail that says what is happening right now, the pane that says where
// the run is, and trellis's newest stderr line. Every figure is one trellis
// reported or this view's own clock, marked * as its own; one that was not
// reported is Dash, never 0 or a guess (#911). The clock is trellis's own
// once it reports one (#906).

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
	// EngineClock is set when the Budget's elapsed time counts on from an
	// elapsed_ms trellis sent (#906), and clear when it is this view's own.
	EngineClock bool
	// Stopping is set once a stop was sent and the run has not ended.
	Stopping bool
	// Paused is set from the loop's paused event to its resumed event or
	// the run's end (#806): only the event says the run is held. PauseAsked
	// is set from a pause sent until that event or the pause's answer: the
	// strip still says RUNNING, and offers resume, which withdraws a pause
	// not yet reached, as trellis counts a pause asked as one.
	Paused, PauseAsked bool
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
	case s.Running && s.Now.Paused:
		return "PAUSED"
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

// nowStrip is the Now strip's row as spans (#911, the NowStrip card): the
// state as a Status, the run's time (clockText), the turn as a Meter
// against the run's ceiling, the pause key, what the run is on (the last
// turn's tool → target when it ran one), and while a question waits the
// keys that answer it. The pause key comes before what the run is on,
// which is the part the row cuts when it runs short.
func nowStrip(s State) []Span {
	gap := Span{Text: "  "}
	out := []Span{{Text: " "}, statusSpan(stateWord(s), nowRole(s))}
	if s.Running || s.Now.Outcome != "" {
		out = append(out, gap, Span{Text: clockText(s)}, gap)
		out = append(out, meter("", turnUsed(s), s.Budget.MaxIterations, meterCellsShort)...)
	}
	if k, ok := pauseKey(s); ok {
		out = append(out, gap, Span{Text: keyRow(k), Role: RoleLabel})
	}
	// Before its first model call a run is on nothing yet, and the strip
	// says nothing for it rather than "none".
	if p := s.Now.Phase; s.Running && p.Kind != "" {
		on := Activity(p)
		if p.Kind == "tool" && p.Target != "" {
			on += " → " + Plain(p.Target)
		}
		out = append(out, gap, Span{Text: on, Role: RoleTool})
	}
	if s.Running && s.Now.Waiting {
		out = append(out, gap, Span{Text: keyRow(key{"y", "accept"}, key{"n", "reject"}), Role: RoleYou})
	}
	return out
}

// pauseKey is the Now strip's one pause control (#857), as the desktop's
// (#837): pause, or resume once a pause is asked or held, while a run is in
// flight and not stopping. Not while a question waits: the box takes the
// keys then.
func pauseKey(s State) (key, bool) {
	switch {
	case !s.Running || s.Now.Stopping || s.Now.Waiting:
		return key{}, false
	case s.Now.Paused || s.Now.PauseAsked:
		return key{"^P", "resume"}, true
	}
	return key{"^P", "pause"}, true
}

// The register dial's faces (#857), the brand book's Ascii and the
// desktop's (#836): one for a screen with no run yet and one for a run
// that has ended, the same face whatever the outcome, since a face that
// differed by outcome would be a status, which play never is (DESIGN.md
// section 3.1).
const (
	FaceEmpty = "(•_•)"
	FaceEnded = "(•‿•)"
)

// playFace is the playful register's face and copy for the Now strip,
// only where the strip has no run to show: before any run, and once one
// has ended. Serious, and while a run is in flight, nothing. It is chosen
// from the strip's state alone and names no figure, state word or outcome.
func playFace(s State) (Span, bool) {
	switch {
	case !s.Playful || s.Running:
		return Span{}, false
	case s.Now.Outcome != "":
		// Ended, as the strip's own word says it.
		return Span{Text: FaceEnded + " that one is over", Role: RolePlay}, true
	}
	return Span{Text: FaceEmpty + " nothing here yet", Role: RolePlay}, true
}

// withFace is the strip with the playful face after it, when the face fits
// beside the strip and what stands on its right (right cells, 0 for
// nothing): the face is the first thing the row gives up, never a key or
// a figure.
func withFace(s State, now []Span, right, w int) []Span {
	f, ok := playFace(s)
	if !ok || spansWidth(now)+2+Width(f.Text)+right+1 > w {
		return now
	}
	return append(append([]Span{}, now...), Span{Text: "  "}, f)
}

// clockText is the run's time. Once trellis reports its own (elapsed_ms,
// #906) it is that figure, counted on between events, and unmarked; until
// then, and for a run whose run_end carried none, it is this view's clock,
// marked * as its own. Either pauses while a question waits on the person.
// A run in flight reads 0:00* until the first figure; a run whose time
// nothing measured, Dash.
func clockText(s State) string {
	switch {
	case s.Budget.ElapsedSecs != nil && s.Now.EngineClock:
		return Clock(*s.Budget.ElapsedSecs)
	case s.Budget.ElapsedSecs != nil:
		return Clock(*s.Budget.ElapsedSecs) + "*"
	case s.Running:
		return Clock(0) + "*"
	}
	return Dash
}

// turnUsed is the turn the strip and the budget count against the run's
// ceiling: the turn the model is being called for, or the last one
// reported. Turns count from 1, so before trellis has numbered one there is
// no figure, and nil says so.
func turnUsed(s State) *uint64 {
	n := s.Budget.Iteration
	if s.Now.Phase.Kind == "model" && s.Now.Phase.Turn > n {
		n = s.Now.Phase.Turn
	}
	if n == 0 {
		return nil
	}
	return &n
}

// where is the Where it is pane: the run's phase, the runtime, the control
// mode and the repository. trellis reports no control mode to a client
// yet, so it is Dash, as anything trellis has not said is.
func where(s State, w, h int) []string {
	runtime := Plain(s.Now.Runtime)
	if runtime == "" {
		runtime = Dash
	}
	root := Dash
	if r := Plain(s.Now.Root); r != "" {
		root = baseName(r)
	}
	phase := Activity(s.Now.Phase)
	if s.Running && s.Now.Paused {
		// The desktop's words for a held run (#837).
		phase = "paused before the next model call"
	}
	rows := []Row{
		{Text: "  phase    " + phase},
		{Text: "  runtime  " + runtime},
		{Text: "  control  " + Dash, Role: RoleLabel},
		{Text: "  root     " + root},
	}
	return paneBox(s, PanelWhere, "where", "", rows, w, h)
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

// askingRows is the rows drawn around the approval box: the Trail and the
// Now strip above it and the status bar below, which stay on screen while
// a question waits, so the person answering sees the run's state and how
// to answer (#850, #851).
const askingRows = 3

// Asking is the screen while a question waits on the person: the Trail
// and the Now strip, the approval box (Modal) over the stage, which covers
// the panes so the keys on screen are the ones that act, and the status
// bar. Below the minimum size the box is drawn alone over the whole
// screen: "terminal too small" in its place would leave a question
// waiting unseen while y and n still answer it.
func Asking(s State, question, diff string, scroll, w, h int) string {
	if !fits(w, h) {
		return Modal(s.Theme, question, diff, scroll, w, h)
	}
	top := line(s.Theme, trail(s, w), w) + "\n" + line(s.Theme, nowStrip(s), w)
	return top + "\n" + Modal(s.Theme, question, diff, scroll, w, h-askingRows) + "\n" + line(s.Theme, statusBar(s, w), w)
}

// AskingLastFrom is ModalLastFrom for the box Asking draws at w by h: the
// furthest line the model scrolls the question's diff to.
func AskingLastFrom(diff string, w, h int) int {
	if !fits(w, h) {
		return ModalLastFrom(diff, w, h)
	}
	return ModalLastFrom(diff, w, h-askingRows)
}
