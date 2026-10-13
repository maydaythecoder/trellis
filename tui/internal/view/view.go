package view

import (
	"fmt"
	"strings"
	"unicode"
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
}

// Render draws the whole screen at width by height.
func Render(s State, w, h int) string {
	if w < MinWidth || h < MinHeight {
		return tooSmall(w, h)
	}
	entry := entryLines(s, w)
	bodyH := h - 1 - 3 - len(entry)
	leftW, rightW := columns(s, w)
	const radiusH = 6
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
	out = append(out, pad(status(s), w))
	for i := 0; i < bodyH; i++ {
		out = append(out, left[i]+right[i])
	}
	out = append(out, box("budget", "", []string{budgetLabel(s, w-2)}, w, 3)...)
	out = append(out, entry...)
	return strings.Join(out, "\n")
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
		n += len(wrap(plain(l), left-2))
	}
	return n
}

func tooSmall(w, h int) string {
	return fmt.Sprintf("terminal too small\n%dx%d — need at least %dx%d", w, h, MinWidth, MinHeight)
}

// status is the one-line bar: TRELLIS, the model and tier, and whether the
// worktree has edits pending.
func status(s State) string {
	model := plain(s.Model)
	if model == "" {
		model = "none"
	}
	tier := plain(s.Tier)
	if tier == "" {
		tier = "probing…"
	}
	worktree := "clean"
	if n := pending(s.Edits); n > 0 {
		worktree = fmt.Sprintf("%d edit(s) pending", n)
	}
	return " TRELLIS   model " + model + "  tier " + tier + "  worktree " + worktree
}

func pending(es []Edit) int {
	n := 0
	for _, e := range es {
		if e.Status == Proposed {
			n++
		}
	}
	return n
}

// transcript draws the transcript, following its end unless scrolled
// back, with the last run's outcome on the bottom border, which does not
// scroll: a run that ended is visible however long its transcript is.
func transcript(s State, w, h int) []string {
	inner := w - 2
	var rows []string
	for _, l := range s.Transcript {
		rows = append(rows, wrap(plain(l), inner)...)
	}
	innerH := h - 2
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
		bottom = "last run: " + plain(s.Outcome)
	}
	return box(title, bottom, rows[start:end], w, h)
}

func edits(s State, w, h int) []string {
	var lines []string
	for _, e := range s.Edits {
		lines = append(lines, "~ "+plain(e.Path))
	}
	return box("edits", "", lines, w, h)
}

func checks(s State, w, h int) []string {
	var lines []string
	for _, c := range s.Checks {
		word := "failed"
		if c.Passed {
			word = "passed"
		}
		lines = append(lines, plain(c.Name)+" "+word)
	}
	return box("checks", "", lines, w, h)
}

// radius draws the three counts as three rows meeting at one focus, never
// as a ratio, a percentage or a total, and says what they are a floor on.
func radius(s State, w, h int) []string {
	if w-2 < 24 {
		return box("radius", "", nil, w, h)
	}
	if s.Radius == nil {
		return box("radius", "", []string{"  no blast_radius call yet"}, w, h)
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
	return box("radius", "", lines, w, h)
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
		n := node{name: plain(name), calls: calls}
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

// widths is the widest label and the widest count among the nodes.
func widths(ns []node) (labelW, countW int) {
	for _, n := range ns {
		labelW = max(labelW, len([]rune(n.name)))
		countW = max(countW, len([]rune(n.count)))
	}
	return labelW, countW
}

// actionsWidth is the columns the graph's widest row takes, so the layout
// can give the pane that much (#655): indent, label, the count between two
// spaces, glyph, edge, and the focus.
func actionsWidth(s State) int {
	labelW, countW := widths(nodes(s))
	return 2 + labelW + 1 + countW + 1 + 1 + 2 + len([]rune(focus(s)))
}

// actions draws the action graph: every tool a node, filled once called,
// edges meeting at one focus, then how much reached the tree, as a count
// and never a share, then the newest actions in what room is left. The
// counts are right-aligned to the widest, so the edge column stays one
// column whatever a node's count says.
func actions(s State, w, h int) []string {
	inner := w - 2
	if inner < 24 {
		return box("actions", "", nil, w, h)
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
		l := "  " + padRunes(n.name, labelW) + " " + strings.Repeat(" ", countW-len([]rune(n.count))) + n.count + " " + glyph + "─" + join
		if i == mid {
			l += focus(s)
		}
		lines = append(lines, cut(l, inner))
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
			lines = append(lines, cut("  "+mark+" "+plain(a.Tool)+" "+plain(a.Target), inner))
		}
	}
	return box("actions", "", lines, w, h)
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
		if width < 0 || len([]rune(join())) <= width {
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
func entryLines(s State, w int) []string {
	lines := []string{plain(s.Input) + "█"}
	title := "task  ·  enter to run  ·  /help  ·  esc to clear, esc again to quit"
	if s.Running {
		title = "running — enter queues a steer for its next turn  ·  ^c stops it"
	} else {
		for i, p := range s.Offered {
			mark := "    "
			if i == 0 {
				mark = "tab "
			}
			lines = append(lines, mark+plain(p))
		}
	}
	return box(title, "", lines, w, 2+len(lines))
}

// box draws lines in a w by h bordered box, the title in the top border and
// bottom in the bottom one. Lines are cut, never wrapped, so a long one
// cannot break the border; a box too small for its lines keeps the first.
func box(title, bottom string, lines []string, w, h int) []string {
	if h < 2 || w < 2 {
		return make([]string, max(h, 0))
	}
	inner := w - 2
	out := []string{edge("┌", "┐", title, inner)}
	for i := 0; i < h-2; i++ {
		l := ""
		if i < len(lines) {
			l = lines[i]
		}
		out = append(out, "│"+padRunes(cut(l, inner), inner)+"│")
	}
	return append(out, edge("└", "┘", bottom, inner))
}

func edge(left, right, title string, inner int) string {
	if title == "" {
		return left + strings.Repeat("─", inner) + right
	}
	t := cut(" "+plain(title)+" ", inner)
	return left + t + strings.Repeat("─", inner-len([]rune(t))) + right
}

// plain removes control characters (C0, DEL, C1), so served text drawn in a
// pane can never move the cursor or start an escape sequence.
func plain(s string) string {
	if strings.IndexFunc(s, unicode.IsControl) < 0 {
		return s
	}
	return strings.Map(func(r rune) rune {
		if unicode.IsControl(r) {
			return -1
		}
		return r
	}, s)
}

func pad(s string, w int) string { return padRunes(cut(s, w), w) }

func padRunes(s string, w int) string {
	n := len([]rune(s))
	if n >= w {
		return s
	}
	return s + strings.Repeat(" ", w-n)
}

func contains(xs []string, x string) bool {
	for _, v := range xs {
		if v == x {
			return true
		}
	}
	return false
}
