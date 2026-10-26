// Package view draws the Go TUI. It holds no state and changes none: each
// function takes what it draws and returns the text, so a test reads the
// screen as a string. It imports nothing of the TUI's own, so the model can
// use it without a cycle.
//
// A frame starts at Render, below, which the model's View calls with the
// State that model/panes.go builds; while an approval question is up, View
// draws Modal (modal.go) instead. Both draw their regions with Box
// (frame.go), from text that the helpers in text.go make plain, cut, pad
// and wrap, measuring every width in terminal cells.
package view

import (
	"fmt"
	"strings"
)

// Below this the six-region layout has no room to be legible; the ratatui
// TUI's minimum.
const (
	MinWidth  = 80
	MinHeight = 24
)

// RightMinWidth is the right column's floor: two borders and the radius
// pane's caption, "a floor on reach, not a boundary", at 34 columns. The
// actions pane's widest row is measured per frame and can raise it (#655,
// the ratatui TUI's RIGHT_MIN_WIDTH).
const RightMinWidth = 36

// TranscriptPage is how many rows PgUp and PgDn move the transcript
// (#660): the arrows move one, and the start of a long run was hundreds of
// presses away.
const TranscriptPage = 10

// Tools the loop can call, in the order the prompt offers them. A fixed list
// rather than a tally of what was called, so the graph shows a tool that was
// available and not used: a run that never read a file before editing it
// looks different from one that did.
var Tools = []string{"blast_radius", "read_file", "edit", "run_checks"}

// EditStatus is where a pending edit stands. Proposed is the one status
// trellis reports: the edit is written to the tree, pending review. An
// approval answer is about the next call, not the edits already shown, so
// nothing relabels them applied or rejected (#635); a file the loop has
// written is in the tree whatever the answer.
type EditStatus int

const (
	Proposed EditStatus = iota
)

type Edit struct {
	Path   string
	Status EditStatus
}

// Check is a check trellis reported running, and what it said. There is no
// way to make one but from a verdict trellis sent (model/panes.go).
type Check struct {
	Name   string
	Passed bool
}

// Action is one thing the agent did, and whether it changed the tree. No
// outcome: whether it was correct is the checks pane's, and only an
// executed check can say (ADR 0004).
type Action struct {
	Tool, Target string
	Wrote        bool
}

// Radius is a blast radius's three counts. They come from different
// populations and are never divided, summed or shown as a share (ADR 0002,
// ADR 0014).
type Radius struct {
	Resolved, Candidates, Unresolved uint64
}

// Budget is the run's progress. A nil figure is one trellis did not report,
// and nothing is drawn for it: never a 0 standing in for it.
type Budget struct {
	Iteration        uint64
	MaxIterations    *uint64
	PromptBytes      *uint64
	ElapsedSecs      *uint64
	CompletionTokens *uint64
	ModelMs          *uint64
	QueueWaitMs      *uint64
}

// State is everything the screen draws. Every string in it is served text
// and is made plain again here before it is drawn.
type State struct {
	Model, Tier string
	Transcript  []string
	ScrollBack  int
	Outcome     string
	Running     bool
	Input       string
	// Offered are the @ completions to draw under the input line.
	Offered []string
	Edits   []Edit
	Checks  []Check
	Actions []Action
	Radius  *Radius
	Budget  Budget
	// Stderr is trellis's stderr, the newest last, for the capped messages
	// line (ADR 0052 section 4).
	Stderr []string
	// Restored is the last run's run_end restored, nil when it did not say
	// or no run has ended since the current one started.
	Restored *bool
}

// radiusH is the radius pane's height: its three counts, the line that
// says what they are a floor on, and two borders.
const radiusH = 6

// MinBodyRows is the fewest rows the body between the status bar and the
// budget keeps, whatever the task box below it holds: the radius pane's,
// so its three counts are never cut, and the transcript keeps four rows.
// The @ completion list takes what is left above it, and no more (#842):
// sixteen completions at 80x24 left the body one row, and the transcript
// sliced past its own end.
const MinBodyRows = radiusH

// Render draws the whole screen at width by height.
func Render(s State, w, h int) string {
	if w < MinWidth || h < MinHeight {
		return tooSmall(w, h)
	}
	entry, bodyH := layout(s, w, h)
	leftW, rightW := columns(s, w)
	rest := bodyH - radiusH
	actionsH := rest / 2
	editsH := rest / 4
	checksH := rest - actionsH - editsH

	left := transcript(s, leftW, bodyH)
	var right []string
	right = append(right, actions(s, rightW, actionsH)...)
	right = append(right, edits(s, rightW, editsH)...)
	right = append(right, checks(s, rightW, checksH)...)
	right = append(right, radius(s, rightW, radiusH)...)

	var out []string
	out = append(out, Pad(status(s, w), w))
	for i := 0; i < bodyH; i++ {
		out = append(out, left[i]+right[i])
	}
	out = append(out, Box("budget", "", []string{budgetLabel(s, w-2)}, w, 3)...)
	out = append(out, entry...)
	return strings.Join(out, "\n")
}

// layout is the task box's rows and the body's height at w by h, for
// Render and for the scroll limits the model keeps (TranscriptTop), so
// the two cannot disagree about how tall the transcript is (#843). The
// status bar, the budget box, and the task box's two borders and its
// input row are always drawn; the completions get what is left of the
// rest once the body has its floor.
func layout(s State, w, h int) (entry []string, bodyH int) {
	entry = entryLines(s, w, h-1-3-3-MinBodyRows)
	return entry, h - 1 - 3 - len(entry)
}

// columns splits the width between the transcript and the right column.
// The right column takes two fifths, and never less than its widest row
// needs: at 80 columns two fifths were 32, and the panes read "0 ac",
// "reac" and "not a boun" where their labels were (#655). Below 100
// columns the transcript gives the difference up, since a cut figure is
// worse than a narrower transcript that wraps.
func columns(s State, w int) (left, right int) {
	right = max(w*2/5, RightMinWidth, actionsWidth(s)+2)
	return w - right, right
}

// TranscriptRows is how many rows the transcript wraps to in the layout
// drawn at width w: the scroll back that shows its first row (Home, #660).
func TranscriptRows(s State, w int) int {
	if w < MinWidth {
		w = MinWidth
	}
	left, _ := columns(s, w)
	n := 0
	for _, l := range s.Transcript {
		n += len(WrapWords(Plain(l), left-2))
	}
	return n
}

// TranscriptTop is the scroll back that shows the transcript's first row
// at the top of its pane, in the layout drawn at w by h: the most Home, PgUp
// and Up go to. The pane shows a page of rows, so the top is the rows less
// that page; counted as all the rows, as it was, the first PgDn after Home
// spent itself on rows above the first and moved nothing (#843).
func TranscriptTop(s State, w, h int) int {
	w, h = max(w, MinWidth), max(h, MinHeight)
	_, bodyH := layout(s, w, h)
	return max(TranscriptRows(s, w)-max(bodyH-2, 0), 0)
}

func tooSmall(w, h int) string {
	return fmt.Sprintf("terminal too small\n%dx%d — need at least %dx%d", w, h, MinWidth, MinHeight)
}

// status is the one-line bar: TRELLIS, the worktree's state, the model and
// the tier. The worktree comes before the model, and the model is the span
// cut, with "…", to the room left: drawn last, the worktree was pushed past
// column 80 by a long spec and read "work" (#636).
func status(s State, w int) string {
	model := Plain(s.Model)
	if model == "" {
		model = "none"
	}
	tier := Plain(s.Tier)
	if tier == "" {
		tier = "probing…"
	}
	head := " TRELLIS   worktree " + WorktreeState(s) + "  model "
	tail := "  tier " + tier
	room := max(w-Width(head)-Width(tail), 1)
	return head + Cut(model, room) + tail
}

// WorktreeState is what the bar says about the worktree (#636), from this
// task's writes and how its run ended. The edits pane holds one row per
// write trellis reported, so a run in flight counts writes and a finished
// one counts the files they touched. A run that rolled back has left
// nothing, and one that did not say so (a converged run, or an ending that
// did not report it) has left its files for the person to review.
func WorktreeState(s State) string {
	if s.Running {
		if n := len(s.Edits); n > 0 {
			return fmt.Sprintf("%d edit(s) pending", n)
		}
		return "clean"
	}
	if s.Restored != nil && *s.Restored {
		return "restored"
	}
	files := map[string]bool{}
	for _, e := range s.Edits {
		files[e.Path] = true
	}
	if len(files) > 0 {
		return fmt.Sprintf("%d file(s) left for review", len(files))
	}
	return "clean"
}

// transcript draws the transcript, following its end unless scrolled
// back, with the last run's outcome on the bottom border, which does not
// scroll: a run that ended is visible however long its transcript is.
func transcript(s State, w, h int) []string {
	inner := w - 2
	var rows []string
	for _, l := range s.Transcript {
		rows = append(rows, WrapWords(Plain(l), inner)...)
	}
	// A pane too short for an inside shows no rows: h-2 below zero made
	// start pass end and the slice panicked (#842).
	innerH := max(h-2, 0)
	// Scrolled back past the first row, the first page is shown: the view
	// stops at the top rather than drawing an empty pane.
	end := max(len(rows)-s.ScrollBack, min(innerH, len(rows)))
	start := end - innerH
	if start < 0 {
		start = 0
	}
	title := "transcript"
	if s.ScrollBack > 0 {
		title = "transcript · scrolled back · ↓ to follow"
	}
	bottom := ""
	if s.Outcome != "" {
		bottom = "last run: " + Plain(s.Outcome)
	}
	return Box(title, bottom, rows[start:end], w, h)
}

func edits(s State, w, h int) []string {
	var lines []string
	for _, e := range s.Edits {
		lines = append(lines, "~ "+Plain(e.Path))
	}
	return Box("edits", "", lines, w, h)
}

func checks(s State, w, h int) []string {
	var lines []string
	for _, c := range s.Checks {
		word := "failed"
		if c.Passed {
			word = "passed"
		}
		lines = append(lines, Plain(c.Name)+" "+word)
	}
	return Box("checks", "", lines, w, h)
}

// radius draws the three counts as three rows meeting at one focus, never
// as a ratio, a percentage or a total, and says what they are a floor on.
func radius(s State, w, h int) []string {
	if w-2 < 24 {
		return Box("radius", "", nil, w, h)
	}
	if s.Radius == nil {
		return Box("radius", "", []string{"  no blast_radius call yet"}, w, h)
	}
	r := s.Radius
	rows := []struct {
		label string
		n     uint64
		glyph string
		join  string
	}{
		{"resolved", r.Resolved, "●", "┐"},
		{"candidates", r.Candidates, "◐", "┤"},
		{"unresolved", r.Unresolved, "○", "┘"},
	}
	var lines []string
	for i, row := range rows {
		l := fmt.Sprintf("  %-11s%5d %s─%s", row.label, row.n, row.glyph, row.join)
		if i == 1 {
			l += "──◆ reach"
		}
		lines = append(lines, l)
	}
	lines = append(lines, "  a floor on reach, not a boundary")
	return Box("radius", "", lines, w, h)
}

// node is one node of the action graph: a tool and its count as its row
// draws it. The count is empty for an uncalled tool, N for one that only
// looked, and "N · M wrote" for one that changed the tree, said in words
// and not only by a glyph (#661), as the desktop's graph says "3 calls · 1
// wrote".
type node struct {
	name, count string
	calls       int
}

// nodes is every tool a node, in the prompt's order, then any tool the
// model called that Tools has not heard of: a node missing because a list
// was out of date would make the picture a lie about what ran.
func nodes(s State) []node {
	names := append([]string(nil), Tools...)
	for _, a := range s.Actions {
		if !contains(names, a.Tool) {
			names = append(names, a.Tool)
		}
	}
	var out []node
	for _, name := range names {
		calls, wrote := 0, 0
		for _, a := range s.Actions {
			if a.Tool == name {
				calls++
				if a.Wrote {
					wrote++
				}
			}
		}
		n := node{name: Plain(name), calls: calls}
		switch {
		case calls == 0:
		case wrote == 0:
			n.count = fmt.Sprint(calls)
		default:
			n.count = fmt.Sprintf("%d · %d wrote", calls, wrote)
		}
		out = append(out, n)
	}
	return out
}

// focus is the label where the edges meet.
func focus(s State) string {
	n := len(s.Actions)
	plural := "s"
	if n == 1 {
		plural = ""
	}
	return fmt.Sprintf("──◆ %d action%s", n, plural)
}

// widths is the widest label and the widest count among the nodes, in
// cells.
func widths(ns []node) (labelW, countW int) {
	for _, n := range ns {
		labelW = max(labelW, Width(n.name))
		countW = max(countW, Width(n.count))
	}
	return labelW, countW
}

// actionsWidth is the cells the graph's widest row takes, so the layout
// can give the pane that much (#655): indent, label, the count between two
// spaces, glyph, edge, and the focus.
func actionsWidth(s State) int {
	labelW, countW := widths(nodes(s))
	return 2 + labelW + 1 + countW + 1 + 1 + 2 + Width(focus(s))
}

// actions draws the action graph: every tool a node, filled once called,
// edges meeting at one focus, then how much reached the tree, as a count
// and never a share, then the newest actions in what room is left. The
// counts are right-aligned to the widest, so the edge column stays one
// column whatever a node's count says.
func actions(s State, w, h int) []string {
	inner := w - 2
	if inner < 24 {
		return Box("actions", "", nil, w, h)
	}
	ns := nodes(s)
	labelW, countW := widths(ns)
	mid := len(ns) / 2
	var lines []string
	for i, n := range ns {
		join := "┤"
		if i == 0 {
			join = "┐"
		} else if i == len(ns)-1 {
			join = "┘"
		}
		glyph := "○"
		if n.calls > 0 {
			glyph = "●"
		}
		l := "  " + Pad(n.name, labelW) + " " + strings.Repeat(" ", countW-Width(n.count)) + n.count + " " + glyph + "─" + join
		if i == mid {
			l += focus(s)
		}
		lines = append(lines, Cut(l, inner))
	}
	wroteN := 0
	for _, a := range s.Actions {
		if a.Wrote {
			wroteN++
		}
	}
	switch {
	case len(s.Actions) == 0:
		lines = append(lines, "  nothing yet")
	case wroteN == 0:
		lines = append(lines, "  nothing written to the tree")
	default:
		lines = append(lines, fmt.Sprintf("  %d wrote to the tree", wroteN))
	}
	room := h - 2 - len(lines)
	if room > 0 && len(s.Actions) > 0 {
		from := len(s.Actions) - room
		if from < 0 {
			from = 0
		}
		for _, a := range s.Actions[from:] {
			mark := "·"
			if a.Wrote {
				mark = "*"
			}
			lines = append(lines, Cut("  "+mark+" "+Plain(a.Tool)+" "+Plain(a.Target), inner))
		}
	}
	return Box("actions", "", lines, w, h)
}

// budgetSegments are the budget line's figures, in order: the iteration
// against the run's own ceiling, the prompt's size, the time elapsed, and
// #257's rate and queue wait (nil when not drawn at all). A figure trellis
// did not report is named as unreported, never drawn as 0 (#641): at
// launch the line used to read "iteration 0 · 0 B prompt · 0s elapsed",
// three figures nothing had measured.
func budgetSegments(s State) (iteration, prompt, elapsed string, rate, queued *string) {
	b := s.Budget
	// Turns count from 1: a 0 is a run that has not reported a turn.
	switch {
	case b.Iteration == 0 && b.MaxIterations == nil:
		iteration = "no iteration yet"
	case b.Iteration == 0:
		iteration = fmt.Sprintf("no iteration yet of %d", *b.MaxIterations)
	case b.MaxIterations == nil:
		iteration = fmt.Sprintf("iteration %d", b.Iteration)
	default:
		iteration = fmt.Sprintf("iteration %d/%d", b.Iteration, *b.MaxIterations)
	}
	// A finished run does not keep presenting an iteration in progress.
	if !s.Running && s.Outcome != "" {
		iteration = "finished · " + iteration
	}
	prompt = "prompt not reported"
	if b.PromptBytes != nil {
		prompt = fmt.Sprintf("%d B prompt", *b.PromptBytes)
	}
	elapsed = "not started"
	if b.ElapsedSecs != nil {
		elapsed = Clock(*b.ElapsedSecs) + " elapsed"
	}
	rate, queued = turnRate(b)
	return iteration, prompt, elapsed, rate, queued
}

// BudgetLine is the budget's whole line, every segment kept.
func BudgetLine(s State) string { return budgetLabel(s, -1) }

// budgetLabel is the budget line joined with "  ·  ", dropping whole
// segments until it fits width (#655); a negative width keeps them all. A
// line cut by the box ended mid-word ("· queu"), so a segment that does not
// fit is left out rather than cut, in this order: the token rate first,
// since it is the one figure derived from the others; then the queue wait;
// then the prompt size. The iteration and the elapsed time are never
// dropped: they are what says the run is moving.
func budgetLabel(s State, width int) string {
	iteration, prompt, elapsed, rate, queued := budgetSegments(s)
	segs := []*string{&iteration, &prompt, &elapsed, rate, queued}
	join := func() string {
		var parts []string
		for _, p := range segs {
			if p != nil {
				parts = append(parts, *p)
			}
		}
		return strings.Join(parts, "  ·  ")
	}
	for _, drop := range []int{3, 4, 1} {
		if width < 0 || Width(join()) <= width {
			break
		}
		segs[drop] = nil
	}
	return join()
}

// Clock is seconds as m:ss, so a minutes-long model call reads as time
// passing rather than as a count (#641): "1:15" where "75s" needed
// arithmetic.
func Clock(secs uint64) string {
	return fmt.Sprintf("%d:%02d", secs/60, secs%60)
}

// turnRate is the last turn's output rate and queue wait (#257), as two
// segments so the line can drop one without the other: output tokens over
// the call's wall time less the queue wait trellis reported (0 when it did
// not). With no tokens reported neither is drawn, and with no model time
// left after the wait there is no rate to draw.
func turnRate(b Budget) (rate, queued *string) {
	if b.CompletionTokens == nil {
		return nil, nil
	}
	if b.ModelMs != nil {
		var wait uint64
		if b.QueueWaitMs != nil {
			wait = *b.QueueWaitMs
		}
		if *b.ModelMs > wait {
			decoding := *b.ModelMs - wait
			r := fmt.Sprintf("%d tok, %.1f tok/s", *b.CompletionTokens, float64(*b.CompletionTokens)*1000/float64(decoding))
			rate = &r
		}
	}
	if b.QueueWaitMs != nil {
		q := fmt.Sprintf("queued %.1f s", float64(*b.QueueWaitMs)/1000)
		queued = &q
	}
	return rate, queued
}

// entryLines is the task entry: the typed line with a block cursor, the @
// candidates under it (none while a run holds the input: what is typed then
// is a steer), and what Enter does now.
// entryLines is the task box: the line being typed and, while no run is
// in flight, the @ completions under it, in at most room rows.
//
// The line keeps its end in view, where the cursor is: wider than the box,
// it was cut at the right and the cursor went with it (#842). What scrolled
// off the left is marked "…".
//
// The completions take at most room rows. When there are more, the last
// row says how many are not shown, so a short list is never read as all
// there is.
func entryLines(s State, w, room int) []string {
	lines := []string{CutLeft(Plain(s.Input)+"█", w-2)}
	title := "task  ·  enter to run  ·  /help  ·  esc to clear, esc again to quit"
	if s.Running {
		title = "running — enter queues a steer for its next turn  ·  ^c stops it"
	} else {
		offered := s.Offered
		hidden := 0
		if len(offered) > room {
			shown := max(room-1, 0)
			hidden = len(offered) - shown
			offered = offered[:shown]
		}
		for i, p := range offered {
			mark := "    "
			if i == 0 {
				mark = "tab "
			}
			lines = append(lines, mark+Plain(p))
		}
		if hidden > 0 && room > 0 {
			lines = append(lines, fmt.Sprintf("    … %d more", hidden))
		}
	}
	return Box(title, "", lines, w, 2+len(lines))
}

func contains(xs []string, x string) bool {
	for _, v := range xs {
		if v == x {
			return true
		}
	}
	return false
}
