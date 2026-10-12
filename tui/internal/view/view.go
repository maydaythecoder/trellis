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

// Tools the loop can call, in the order the prompt offers them. A fixed list
// rather than a tally of what was called, so the graph shows a tool that was
// available and not used: a run that never read a file before editing it
// looks different from one that did.
var Tools = []string{"blast_radius", "read_file", "edit", "run_checks"}

// EditStatus is where a pending edit stands.
type EditStatus int

const (
	Proposed EditStatus = iota
	Applied
	Rejected
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
	leftW := w * 60 / 100
	rightW := w - leftW
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
	out = append(out, box("budget", "", []string{BudgetLine(s)}, w, 3)...)
	out = append(out, entry...)
	return strings.Join(out, "\n")
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
	end := len(rows) - s.ScrollBack
	if end < 0 {
		end = 0
	}
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
		mark := "~"
		switch e.Status {
		case Applied:
			mark = "+"
		case Rejected:
			mark = "-"
		}
		lines = append(lines, mark+" "+plain(e.Path))
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

// actions draws the action graph: every tool a node, filled once called,
// edges meeting at one focus, then how much reached the tree, as a count
// and never a share, then the newest actions in what room is left.
func actions(s State, w, h int) []string {
	inner := w - 2
	if inner < 24 {
		return box("actions", "", nil, w, h)
	}
	names := append([]string(nil), Tools...)
	for _, a := range s.Actions {
		if !contains(names, a.Tool) {
			names = append(names, a.Tool)
		}
	}
	labelW := 0
	for _, n := range names {
		if l := len([]rune(plain(n))); l > labelW {
			labelW = l
		}
	}
	mid := len(names) / 2
	var lines []string
	for i, name := range names {
		calls := 0
		for _, a := range s.Actions {
			if a.Tool == name {
				calls++
			}
		}
		join := "┤"
		if i == 0 {
			join = "┐"
		} else if i == len(names)-1 {
			join = "┘"
		}
		glyph, count := "○", "     "
		if calls > 0 {
			glyph, count = "●", fmt.Sprintf("%4d ", calls)
		}
		l := "  " + padRunes(plain(name), labelW) + count + glyph + "─" + join
		if i == mid {
			n := len(s.Actions)
			plural := "s"
			if n == 1 {
				plural = ""
			}
			l += fmt.Sprintf("──◆ %d action%s", n, plural)
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

// BudgetLine is the budget's one line: the iteration against the run's own
// ceiling, the prompt's bytes, the time elapsed, and #257's rate and queue
// wait. A figure trellis did not report is left out, never drawn as 0.
func BudgetLine(s State) string {
	b := s.Budget
	it := fmt.Sprintf("iteration %d", b.Iteration)
	if b.MaxIterations != nil {
		it = fmt.Sprintf("iteration %d/%d", b.Iteration, *b.MaxIterations)
	}
	// A finished run does not keep presenting an iteration in progress.
	if !s.Running && s.Outcome != "" {
		it = "finished · " + it
	}
	parts := []string{it}
	if b.PromptBytes != nil {
		parts = append(parts, fmt.Sprintf("%d B prompt", *b.PromptBytes))
	}
	if b.ElapsedSecs != nil {
		parts = append(parts, fmt.Sprintf("%ds elapsed", *b.ElapsedSecs))
	}
	return strings.Join(parts, "  ·  ") + turnRate(b)
}

// turnRate is the last turn's output rate and queue wait (#257): output
// tokens over the call's wall time less the queue wait trellis reported (0
// when it did not). With no tokens reported the tail is empty, and with no
// model time left after the wait there is no rate to draw.
func turnRate(b Budget) string {
	if b.CompletionTokens == nil {
		return ""
	}
	tail := ""
	if b.ModelMs != nil {
		var wait uint64
		if b.QueueWaitMs != nil {
			wait = *b.QueueWaitMs
		}
		if *b.ModelMs > wait {
			decoding := *b.ModelMs - wait
			rate := float64(*b.CompletionTokens) * 1000 / float64(decoding)
			tail += fmt.Sprintf("  ·  %d tok, %.1f tok/s", *b.CompletionTokens, rate)
		}
	}
	if b.QueueWaitMs != nil {
		tail += fmt.Sprintf("  ·  queued %.1f s", float64(*b.QueueWaitMs)/1000)
	}
	return tail
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
