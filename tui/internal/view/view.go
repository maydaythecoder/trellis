// Package view draws the Go TUI. It holds no state and changes none: each
// function takes what it draws and returns the text, so a test reads the
// screen as a string. It imports nothing of the TUI's own but the theme it
// paints with, so the model can use it without a cycle.
//
// A frame starts at Render, below, which the model's View calls with the
// State that model/panes.go builds; while an approval question is up, View
// draws Modal (modal.go) instead. Both draw their regions with Box
// (frame.go), from text that the helpers in text.go make plain, cut, pad
// and wrap, measuring every width in terminal cells, and paint each row by
// its role last (paint.go).
package view

import (
	"fmt"
	"strings"

	"github.com/maydaythecoder/trellis/tui/internal/theme"
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

// MinTranscriptWidth is the transcript's floor: the right column never
// takes more than the width less this, so a custom tool's long name is cut
// in its node instead of pushing the column off the screen (#875). It is
// two fifths of MinWidth: at 80 columns the four built-in tools' rows need
// up to 48 cells in a run of under a hundred actions, and every built-in
// label stays whole there.
const MinTranscriptWidth = MinWidth * 2 / 5

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

// Edit is one write trellis reported, from its turn event: the path the
// turn targeted and, for an edit that applied, its line counts, where its
// hunk starts and the hunk itself (#802). Each is nil where the turn
// carried none: a file write_file created has no counts and no diff.
type Edit struct {
	Path   string
	Status EditStatus
	Turn   uint64
	// Added and Removed are the whole lines the edit added and removed.
	Added, Removed *uint64
	// Diff is trellis's unified diff of the hunk, kept as sent: repository
	// text and model output, made plain line by line when it is drawn.
	Diff *string
}

// Check is a check trellis reported running, and what it said. There is no
// way to make one but from a verdict trellis sent (model/panes.go).
type Check struct {
	Name   string
	Passed bool
	// Turn is the turn that ran it, and TookMs that turn's tool call time
	// (duration_ms), nil when the turn carried none (#854).
	Turn   uint64
	TookMs *uint64
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
	// The run's other ceilings, from run budget, and the completion tokens
	// spent against MaxTokens, from the last turn (#854). The wall clock's
	// ceiling is in milliseconds, as trellis sends it.
	MaxWallClockMs *uint64
	MaxTokens      *uint64
	ContextBytes   *uint64
	TokensUsed     *uint64
}

// Dropped is how many of a run pane's oldest rows the model no longer
// holds: it keeps the newest of each, and the pane says how many went
// (#848).
type Dropped struct {
	Actions, Edits, Checks int
}

// State is everything the screen draws. Every string in it is served text
// and is made plain again here before it is drawn.
type State struct {
	Model, Tier string
	// Transcript is the transcript's rows, oldest first, each with the
	// role its writer gave it (model/rows.go).
	Transcript []Row
	ScrollBack int
	Outcome    string
	Running    bool
	Input      string
	// Offered are the @ completions to draw under the input line.
	Offered []string
	Edits   []Edit
	Checks  []Check
	Actions []Action
	// Dropped counts the rows of Edits, Checks and Actions the model no
	// longer holds.
	Dropped Dropped
	Radius  *Radius
	Budget  Budget
	// Stderr is trellis's stderr, the newest last, for the capped messages
	// line (ADR 0052 section 4).
	Stderr []string
	// Restored is the last run's run_end restored, nil when it did not say
	// or no run has ended since the current one started.
	Restored *bool
	// Theme is the colours each row is painted in, built once from the
	// terminal (cmd/trellis-tui). The zero Theme paints nothing.
	Theme theme.Theme
	// Now is what the Now strip and Where it is draw (now.go, #850).
	Now Now
	// Nav is where the person is in the run view (shell.go, #851).
	Nav Nav
	// Asked is every approval question waiting on the person, the one on
	// screen first, for the code level's state words and its in-place
	// answer (diff.go, #853).
	Asked []Asked
	// Reviewing is set while the question on screen has been set aside to
	// read its file's diff at the code level, where it is answered.
	Reviewing bool
	// Compose is the verb, the held line and the queue the task box draws
	// (compose.go, #852).
	Compose Compose
	// Summary is what the summary level draws that the panes do not hold
	// (summary.go, #854).
	Summary Summary
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
	if !fits(w, h) {
		return tooSmall(w, h)
	}
	entry, bodyH := layout(s, w, h)
	var out []string
	out = append(out, line(s.Theme, trail(s, w), w))
	now := nowStrip(s)
	if !folded(w, h) && s.Nav.Open == "" {
		now = spread(now, summaryWay(s), w)
	}
	out = append(out, line(s.Theme, now, w))
	out = append(out, stage(s, folded(w, h), w, bodyH)...)
	if m := messages(s, w); m != "" {
		out = append(out, m)
	}
	out = append(out, Box(s.Theme, "budget", "", []Row{{Spans: budgetRow(s, w-2)}}, w, 3)...)
	out = append(out, entry...)
	out = append(out, line(s.Theme, statusBar(s, w), w))
	return strings.Join(out, "\n")
}

// layout is the task box's rows and the stage's height at w by h, for
// Render and for the scroll limits the model keeps (TranscriptTop), so
// the two cannot disagree about how tall the transcript is (#843). The
// Trail, the Now strip, the budget box, the status bar, and the task
// box's two borders and its input row are always drawn, and the messages
// row once trellis has written to stderr; the completions get what is
// left of the rest once the stage has its floor.
func layout(s State, w, h int) (entry []string, bodyH int) {
	entry = entryLines(s, w, completionRoom(s, h))
	return entry, h - fixedRows(s) - len(entry)
}

// fixedRows is every row but the stage's and the task box's: the Trail,
// the Now strip, the budget box and the status bar, and the messages row
// once trellis has written to stderr.
func fixedRows(s State) int {
	n := 1 + 1 + 3 + 1
	if len(s.Stderr) > 0 {
		n++
	}
	return n
}

// completionRoom is the most rows the @ completions take at height h: what
// is left once the fixed rows, the task box's borders and input row, and
// the body's floor are drawn.
func completionRoom(s State, h int) int {
	return h - fixedRows(s) - 3 - MinBodyRows
}

// minPaneRows is the most the actions, edits and checks panes give up to
// Where it is: below it, the actions graph loses its own node rows.
const minPaneRows = 12

// nowRole is the Now strip's colour: what needs the person in signal,
// a stop on its way in warn, a run in flight in cool, the engine's own
// activity, and an idle or ended view as a label. An ended run is never
// ok or down: those colours come only from a check's verdict.
func nowRole(s State) Role {
	switch {
	case s.Running && s.Now.Stopping:
		return RoleNote
	case s.Running && s.Now.Waiting:
		return RoleYou
	case s.Running:
		return RoleTool
	}
	return RoleLabel
}

// columns splits the width between the transcript and the right column.
// The right column takes two fifths, and never less than its widest row
// needs: at 80 columns two fifths were 32, and the panes read "0 ac",
// "reac" and "not a boun" where their labels were (#655). Below 100
// columns the transcript gives the difference up, since a cut figure is
// worse than a narrower transcript that wraps.
func columns(s State, w int) (left, right int) {
	right = max(w*2/5, RightMinWidth, actionsWidth(s)+2)
	right = min(right, w-MinTranscriptWidth)
	return w - right, right
}

// transcriptW is the transcript pane's width at the top level of a w by h
// screen: the whole stage once the right column folds (#851), and what the
// column leaves otherwise.
func transcriptW(s State, w, h int) int {
	if folded(w, h) {
		return w
	}
	left, _ := columns(s, w)
	return left
}

// TranscriptRows is how many rows the transcript wraps to in the layout
// drawn at w by h: the scroll back that shows its first row (Home, #660).
func TranscriptRows(s State, w, h int) int {
	w, h = max(w, MinWidth), max(h, MinHeight)
	inner := transcriptW(s, w, h) - 2
	n := 0
	for _, r := range s.Transcript {
		n += len(WrapWords(Plain(r.Text), inner))
	}
	return n
}

// TranscriptTop is the scroll back that shows the transcript's first row
// at the top of its pane, in the layout drawn at w by h: the most Home, PgUp
// and Up go to. The pane shows a page of rows, so the top is the rows less
// that page; counted as all the rows, as it was, the first PgDn after Home
// spent itself on rows above the first and moved nothing (#843). Folded,
// the panes' row under the pane takes one of the stage's rows.
func TranscriptTop(s State, w, h int) int {
	w, h = max(w, MinWidth), max(h, MinHeight)
	_, bodyH := layout(s, w, h)
	if folded(w, h) {
		bodyH--
	}
	return max(TranscriptRows(s, w, h)-max(bodyH-2, 0), 0)
}

// fits reports whether w by h is room for the whole screen. Below it
// Render draws tooSmall, and Asking the approval box alone.
func fits(w, h int) bool { return w >= MinWidth && h >= MinHeight }

func tooSmall(w, h int) string {
	return fmt.Sprintf("terminal too small\n%dx%d — need at least %dx%d", w, h, MinWidth, MinHeight)
}

// barRight is the status bar's right side, w cells at most: the
// worktree's state as a Status, the model and the tier. The worktree comes
// before the model, and the model is the span cut, with "…", to the room
// left: drawn last, the worktree was pushed past column 80 by a long spec
// and read "work" (#636).
func barRight(s State, w int) []Span {
	head := statusSpan(worktree(s))
	model := Plain(s.Model)
	if model == "" {
		model = "none"
	}
	tier := Plain(s.Tier)
	if tier == "" {
		tier = "probing…"
	}
	tail := "  tier " + tier
	room := max(w-Width(head.Text)-Width("  model ")-Width(tail), 1)
	return []Span{head, {Text: "  model " + Cut(model, room) + tail, Role: RoleLabel}}
}

// worktree is the worktree's state as the status bar's Status, its word
// and its tone (#636, #911), from this task's writes and how its run
// ended. The edits pane holds one row per write trellis reported, so a run
// in flight counts writes, pending in the engine's colour, and a finished
// one counts the files they touched, left for the person to review in
// theirs. A run that rolled back has left nothing, and one that did not
// say so (a converged run, or an ending that did not report it) has left
// its files for review. Once the oldest writes are no longer held (#848),
// their files cannot all be counted, and the count ends in "+".
func worktree(s State) (word string, tone Role) {
	if s.Running {
		if n := len(s.Edits) + s.Dropped.Edits; n > 0 {
			return fmt.Sprintf("%d pending", n), RoleTool
		}
		return "clean", RoleLabel
	}
	if s.Restored != nil && *s.Restored {
		return "restored", RoleLabel
	}
	files := map[string]bool{}
	for _, e := range s.Edits {
		files[e.Path] = true
	}
	switch {
	case s.Dropped.Edits > 0:
		return fmt.Sprintf("%d+ to review", len(files)), RoleYou
	case len(files) > 0:
		return fmt.Sprintf("%d to review", len(files)), RoleYou
	}
	return "clean", RoleLabel
}

// notKept is the row a pane draws first when the model no longer holds
// its n oldest rows, or nothing when it holds them all.
func notKept(n int, what string) []Row {
	if n == 0 {
		return nil
	}
	return []Row{{Text: fmt.Sprintf("… %d earlier %s not kept", n, what), Role: RoleLabel}}
}

// transcript draws the transcript, following its end unless scrolled
// back, with the last run's outcome on the bottom border, which does not
// scroll: a run that ended is visible however long its transcript is.
//
// Each row is painted in the role its writer gave it (model/rows.go, #849),
// and every row it wraps to takes the same role. The view never reads a
// role from a row's words: the model's words could then pass for the
// person's or for a verdict.
func transcript(s State, w, h int) []string {
	inner := w - 2
	var rows []Row
	for _, r := range s.Transcript {
		rows = append(rows, rowsOf(r.Role, WrapWords(Plain(r.Text), inner)...)...)
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
	return Box(s.Theme, title, bottom, rows[start:end], w, h)
}

// checks draws a row per check trellis reported, in ok or down from its
// verdict: Check.Passed is set only from one (model/panes.go), and a row is
// never coloured from what its words say.
func checks(s State, w, h int) []string {
	rows := notKept(s.Dropped.Checks, "checks")
	for _, c := range s.Checks {
		r := Row{Text: Plain(c.Name) + " failed", Role: RoleFailed}
		if c.Passed {
			r = Row{Text: Plain(c.Name) + " passed", Role: RolePassed}
		}
		rows = append(rows, r)
	}
	return paneBox(s, PanelChecks, "checks", "", rows, w, h)
}

// radius draws the three counts as three rows meeting at one focus, never
// as a ratio, a percentage or a total, and says what they are a floor on.
func radius(s State, w, h int) []string {
	if w-2 < 24 {
		return paneBox(s, PanelRadius, "radius", "", nil, w, h)
	}
	if s.Radius == nil {
		return paneBox(s, PanelRadius, "radius", "", rowsOf(RoleLabel, "  no blast_radius call yet"), w, h)
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
	var lines []Row
	for i, row := range rows {
		l := fmt.Sprintf("  %-11s%5d %s─%s", row.label, row.n, row.glyph, row.join)
		if i == 1 {
			l += "──◆ reach"
		}
		lines = append(lines, Row{Text: l})
	}
	lines = append(lines, Row{Text: "  a floor on reach, not a boundary", Role: RoleLabel})
	return paneBox(s, PanelRadius, "radius", "", lines, w, h)
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
// can give the pane that much (#655).
func actionsWidth(s State) int {
	labelW, countW := widths(nodes(s))
	return labelW + besideLabel(s, countW)
}

// besideLabel is the cells a node's row takes besides its label: the
// indent, the count between two spaces, glyph, edge, and the focus that
// the middle row carries.
func besideLabel(s State, countW int) int {
	return 2 + 1 + countW + 1 + 1 + 2 + Width(focus(s))
}

// actions draws the action graph: every tool a node, filled once called,
// edges meeting at one focus, then how much reached the tree, as a count
// and never a share, then the newest actions in what room is left. The
// counts are right-aligned to the widest, so the edge column stays one
// column whatever a node's count says.
func actions(s State, w, h int) []string {
	inner := w - 2
	if inner < 24 {
		return paneBox(s, PanelActions, "actions", "", nil, w, h)
	}
	ns := nodes(s)
	labelW, countW := widths(ns)
	// A label that does not fit is cut, so a long name costs its own tail
	// and the counts, the edge column and the focus keep their places
	// (#875).
	labelW = min(labelW, max(1, inner-besideLabel(s, countW)))
	mid := len(ns) / 2
	var lines []Row
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
		l := "  " + Pad(Cut(n.name, labelW), labelW) + " " + strings.Repeat(" ", countW-Width(n.count)) + n.count + " " + glyph + "─" + join
		if i == mid {
			l += focus(s)
		}
		lines = append(lines, Row{Text: Cut(l, inner), Role: RoleTool})
	}
	wroteN := 0
	for _, a := range s.Actions {
		if a.Wrote {
			wroteN++
		}
	}
	// ✎ n is how many of the actions held wrote to the tree (#911), as the
	// recent actions below mark each one that did; /help says so.
	if len(s.Actions) == 0 {
		lines = append(lines, Row{Text: "  nothing yet", Role: RoleLabel})
	} else {
		lines = append(lines, Row{Text: fmt.Sprintf("  ✎ %d", wroteN), Role: RoleLabel})
	}
	// The graph counts the actions held; the ones before them are said,
	// not counted, so every figure in the pane is of the same actions.
	if n := s.Dropped.Actions; n > 0 {
		lines = append(lines, Row{Text: fmt.Sprintf("  %d earlier actions not kept", n), Role: RoleLabel})
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
				mark = "✎"
			}
			lines = append(lines, Row{Text: Cut("  "+mark+" "+Plain(a.Tool)+" "+Plain(a.Target), inner), Role: RoleTool})
		}
	}
	return paneBox(s, PanelActions, "actions", "", lines, w, h)
}

// budgetSegments are the budget row's segments, in order (#911): a Meter
// for the turns against the run's max_iterations (the summary level draws
// every other ceiling, #854);
// the prompt's size in kB; the time by this view's clock; and #257's rate
// and queue wait, nil when not drawn at all. A figure trellis did not
// report is Dash, never 0 (#641): at launch the line used to read
// "iteration 0 · 0 B prompt · 0s elapsed", three figures nothing had
// measured.
func budgetSegments(s State) [][]Span {
	b := s.Budget
	labelled := func(label, value string) []Span {
		return []Span{{Text: label + " ", Role: RoleLabel}, {Text: value}}
	}
	rate, queued := turnRate(b)
	segs := [][]Span{
		meter("TURNS", turnUsed(s), b.MaxIterations, meterCells),
		labelled("PROMPT", kB(b.PromptBytes)),
		labelled("TIME", clockText(s)),
		nil,
		nil,
	}
	if rate != nil {
		segs[3] = []Span{{Text: *rate}}
	}
	if queued != nil {
		segs[4] = []Span{{Text: *queued}}
	}
	return segs
}

// BudgetLine is the budget's whole row as text, every segment kept.
func BudgetLine(s State) string { return spansText(budgetRow(s, -1)) }

// budgetRow is the budget row's segments joined with "  ·  ", dropping
// whole segments until it fits width (#655); a negative width keeps them
// all. A row cut by the box ended mid-word ("· queu"), so a segment that
// does not fit is left out rather than cut, in this order: the token rate
// first, since it is the one figure derived from the others; then the
// queue wait; then the prompt size. The turns and the time are never
// dropped: they are what says the run is moving.
func budgetRow(s State, width int) []Span {
	segs := budgetSegments(s)
	join := func() []Span {
		var out []Span
		for _, seg := range segs {
			if seg == nil {
				continue
			}
			if out != nil {
				out = append(out, Span{Text: "  ·  ", Role: RoleLabel})
			}
			out = append(out, seg...)
		}
		return out
	}
	for _, drop := range []int{3, 4, 1} {
		if width < 0 || spansWidth(join()) <= width {
			break
		}
		segs[drop] = nil
	}
	return join()
}

// kB is a byte count in kilobytes to one place, the unit after a space as
// the design system sets a figure; under 1 kB it stays in bytes, where a
// rounded 0.0 kB would read as nothing. Dash when trellis did not report it.
func kB(n *uint64) string {
	switch {
	case n == nil:
		return Dash
	case *n < 1000:
		return fmt.Sprintf("%d B", *n)
	}
	return fmt.Sprintf("%.1f kB", float64(*n)/1000)
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

// entryLines is the task box: the typed line with a block cursor, the @
// candidates under it (none while a run holds the input: what is typed
// then is sent under the verb), then the queue (compose.go), together in at
// most room rows; what ⏎ does now on its top border, and the verb Tab
// changes to on its bottom one.
//
// The line keeps its end in view, where the cursor is: wider than the box,
// it was cut at the right and the cursor went with it (#842). What scrolled
// off the left is marked "…".
//
// The completions take at most room rows. When there are more, the last
// row says how many are not shown, so a short list is never read as all
// there is. The queue takes what they leave, and says the same.
func entryLines(s State, w, room int) []string {
	lines := []Row{{Text: CutLeft(Plain(s.Input)+"█", w-2), Role: RoleYou}}
	if !s.Running {
		offered := s.Offered
		hidden := 0
		if len(offered) > room {
			shown := max(room-1, 0)
			hidden = len(offered) - shown
			offered = offered[:shown]
		}
		// Tab inserts the first path: its keycap marks it, and the rest
		// line up under it.
		first := Keycap("TAB") + " "
		indent := strings.Repeat(" ", Width(first))
		for i, p := range offered {
			mark := indent
			if i == 0 {
				mark = first
			}
			lines = append(lines, Row{Text: mark + Plain(p), Role: RoleLabel})
		}
		if hidden > 0 && room > 0 {
			lines = append(lines, Row{Text: fmt.Sprintf("%s… %d more", indent, hidden), Role: RoleLabel})
		}
	}
	lines = append(lines, queueRows(s.Compose.Queue, room-(len(lines)-1))...)
	return Box(s.Theme, composeTitle(s), composeBottom(s), lines, w, 2+len(lines))
}

func contains(xs []string, x string) bool {
	for _, v := range xs {
		if v == x {
			return true
		}
	}
	return false
}
