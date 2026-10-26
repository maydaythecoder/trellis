package view

import (
	"fmt"
	"strings"
)

// The summary level (#854), the terminal face of the desktop's summary
// (#832, desktop/src/state.rs `summary`): the run's pane gone into from
// the panes' row, the Now strip's way in, or Tab. While the run is in
// flight it is "so far" and has no outcome, since a run in flight has
// none; once run_end has come it is how the run ended, in run_end's own
// words and exit code. Every part is what the model already holds for
// another pane, drawn again: the checks as trellis-checks reported them,
// by kind with each turn's time; the radius as its three counts (CLAUDE.md
// rule 3); the code level's files; the budget against each ceiling; this
// run's steers with their delivery. Verified and Reviewed are counted only
// in the trail, which this view does not read, so each is its own row with
// its own Dash and the reason (rule 4). The model's closing words are
// fenced under a label saying they are its words, not a verdict, and made
// plain line by line (rule 10).

// Summary is what the summary level draws that the panes do not hold.
type Summary struct {
	// RunID is the trail run_end named; "" until one has.
	RunID string
	// Steers are this run's steers, oldest first, each with its delivery
	// as the composer's queue words it.
	Steers []Queued
	// Closing is the model's closing words, or why there are none.
	Closing Closing
}

// Closing is the model's closing words: Words as trellis sent them, cut by
// the model past its cap, with Cut of Total bytes dropped; or, with Words
// nil, Why there are none.
type Closing struct {
	Words      *string
	Why        string
	Cut, Total int
}

// summaryLabel is the label over the closing words: whose they are.
const summaryLabel = "THE MODEL'S CLOSING WORDS · its own, not a verdict"

// summaryEnded is whether run_end has come for the run the panes hold: the
// transcript's outcome is set at run_end and cleared when a run starts.
func summaryEnded(s State) bool { return !s.Running && s.Outcome != "" }

// summaryTitle is the level's title: so far, how it ended, or no run.
func summaryTitle(s State) string {
	switch {
	case s.Running:
		return "summary · so far"
	case summaryEnded(s):
		return "summary · how the run ended"
	}
	return "summary · no run yet"
}

// wrapped is text as rows of at most w cells, every row after the first
// under the first's text: indent, then lead before the first row only.
func wrapped(indent, lead, text string, role Role, w int) []Row {
	pad := strings.Repeat(" ", Width(lead))
	var out []Row
	for i, part := range WrapWords(text, max(w-Width(indent)-Width(lead), 1)) {
		first := pad
		if i == 0 {
			first = lead
		}
		out = append(out, Row{Text: indent + first + part, Role: role})
	}
	return out
}

// heading is a section's head row, after a blank row that sets it apart
// from the section above.
func heading(text string) []Row {
	return []Row{{}, {Text: " " + text, Role: RoleLabel}}
}

// unreportedRows is a figure trellis did not report: its label, Dash, and
// why, wrapped under itself.
func unreportedRows(label, why string, w int) []Row {
	return wrapped("   ", Pad(label, 10)+Dash+"  ", why, RoleLabel, w)
}

// summaryEnd is the stage and, once run_end has come, its outcome word and
// exit code; before it, the two are Dash with why.
func summaryEnd(s State, w int) []Row {
	switch {
	case s.Running:
		rows := []Row{{Text: " so far · the run is in flight, and this is not its end", Role: RoleTool}}
		why := "the run is in flight: its outcome and exit code come with run_end"
		rows = append(rows, unreportedRows("outcome", why, w)...)
		return append(rows, unreportedRows("exit", why, w)...)
	case !summaryEnded(s):
		rows := []Row{{Text: " no run yet", Role: RoleLabel}}
		return append(rows, unreportedRows("outcome", "no run has started", w)...)
	}
	rows := []Row{{Text: " how the run ended", Role: RoleLabel}}
	// An ended run is never ok or down: those colours are a check's alone
	// (paint.go), as the Now strip draws the ending.
	if s.Now.Outcome == "" {
		rows = append(rows, unreportedRows("outcome", "run_end carried no outcome", w)...)
	} else {
		rows = append(rows, Row{Spans: []Span{{Text: "   " + Pad("outcome", 10)}, statusSpan(Plain(s.Now.Outcome), RoleLabel)}})
	}
	if s.Now.Exit == nil {
		rows = append(rows, unreportedRows("exit", "run_end carried no exit code", w)...)
	} else {
		rows = append(rows, Row{Text: fmt.Sprintf("   %s%d", Pad("exit", 10), *s.Now.Exit)})
	}
	if s.Restored != nil {
		word := "not restored"
		if *s.Restored {
			word = "restored"
		}
		rows = append(rows, Row{Text: "   " + Pad("tree", 10) + word})
	}
	return rows
}

// summaryChecks is the checks by kind, in the order each kind first ran,
// and under each kind every run of it: its turn, the verdict trellis sent,
// in ok or down from that verdict alone (ADR 0004), and its turn's time.
func summaryChecks(s State, w int) []Row {
	rows := heading("CHECKS BY KIND · trellis-checks' verdict and the turn's time")
	rows = append(rows, notKept(s.Dropped.Checks, "checks")...)
	if len(s.Checks) == 0 {
		return append(rows, Row{Text: "   no check has run", Role: RoleLabel})
	}
	var kinds []string
	for _, c := range s.Checks {
		if !contains(kinds, c.Name) {
			kinds = append(kinds, c.Name)
		}
	}
	for _, k := range kinds {
		rows = append(rows, Row{Text: "   " + Plain(k)})
		for _, c := range s.Checks {
			if c.Name != k {
				continue
			}
			verdict, role := "failed", RoleFailed
			if c.Passed {
				verdict, role = "passed", RolePassed
			}
			took := Dash + "  the turn's event carried no duration_ms"
			if c.TookMs != nil {
				took = fmt.Sprintf("%d ms", *c.TookMs)
			}
			rows = append(rows, wrapped("     ", fmt.Sprintf("turn %d  %s  ", c.Turn, verdict), took, role, w)...)
		}
	}
	return rows
}

// summaryModes is Verified and Reviewed: two rows, each its own figure,
// never added together and never one a share of the other (CLAUDE.md rule
// 4). Both are counted only in the trail trellis writes at run_end, and
// trellis rpc sends neither, so each is Dash with why; the trail's id is
// named for `trellis log`, which lists both.
func summaryModes(s State, w int) []Row {
	why := "no run has started"
	switch {
	case s.Running:
		why = "counted in the trail, which trellis writes at run_end"
	case summaryEnded(s) && s.Summary.RunID == "":
		why = "run_end named no trail: trellis wrote none for this run"
	case summaryEnded(s):
		why = "trellis rpc sends no mode counts, and this view reads no trail; `trellis log` lists run " + Plain(s.Summary.RunID)
	}
	rows := heading("VERIFIED AND REVIEWED · from the trail, two figures")
	rows = append(rows, unreportedRows("Verified", why, w)...)
	return append(rows, unreportedRows("Reviewed", why, w)...)
}

// summaryRadius is the last blast radius as its three counts, never a
// ratio, a share or a total (CLAUDE.md rule 3).
func summaryRadius(s State) []Row {
	rows := heading("RADIUS")
	if s.Radius == nil {
		return append(rows, Row{Text: "   no blast_radius turn has reported one this run", Role: RoleLabel})
	}
	r := s.Radius
	return append(rows,
		Row{Text: fmt.Sprintf("   %s%d", Pad("resolved", 12), r.Resolved)},
		Row{Text: fmt.Sprintf("   %s%d", Pad("candidates", 12), r.Candidates)},
		Row{Text: fmt.Sprintf("   %s%d", Pad("unresolved", 12), r.Unresolved)},
		Row{Text: "   a floor on reach, not a boundary", Role: RoleLabel})
}

// summaryFiles is the code level's files: path, two line counts and the
// state word. The path is cut first, so the counts and the word keep their
// places.
func summaryFiles(s State, w int) []Row {
	rows := heading("FILES")
	rows = append(rows, notKept(s.Dropped.Edits, "edits")...)
	files := codeFiles(s)
	if len(files) == 0 {
		return append(rows, Row{Text: "   this run has written no file", Role: RoleLabel})
	}
	for _, f := range files {
		right := append(counts(f), Span{Text: "  "}, statusSpan(f.word, f.tone))
		room := max(w-spansWidth(right)-3-1, 1)
		rows = append(rows, Row{Spans: spread([]Span{{Text: "   " + Cut(Plain(f.path), room)}}, right, w)})
	}
	return rows
}

// limitRow is one budget row: its label, a Meter's cells when both figures
// are reported, the readout used/ceiling, and why a figure is missing on
// the rows under it.
func limitRow(label string, used, ceiling *uint64, readout, why string, w int) []Row {
	spans := []Span{{Text: "   " + Pad(label, 13), Role: RoleLabel}}
	if used != nil && ceiling != nil {
		on := meterFilled(*used, *ceiling, meterCells)
		spans = append(spans,
			Span{Text: strings.Repeat("█", on), Role: meterRole(*used, *ceiling)},
			Span{Text: strings.Repeat("░", meterCells-on), Role: RoleMeterEmpty},
			Span{Text: " "})
	}
	rows := []Row{{Spans: append(spans, Span{Text: readout})}}
	if why != "" {
		rows = append(rows, wrapped("      ", "", why, RoleLabel, w)...)
	}
	return rows
}

// summaryBudget is the budget used against each ceiling the run budget
// sent: the turns, the wall clock, the completion tokens and the last
// prompt's bytes. Prompt and completion are never summed (UI rule 5).
func summaryBudget(s State, w int) []Row {
	b := s.Budget
	rows := heading("BUDGET · used/ceiling")
	noBudget := b.MaxIterations == nil && b.MaxWallClockMs == nil && b.MaxTokens == nil
	if noBudget && (s.Running || summaryEnded(s)) {
		rows = append(rows, wrapped("   ", "", "trellis has sent no run budget for this run", RoleLabel, w)...)
	}
	for _, l := range Limits {
		label, used, ceiling, readout := limitFigures(s, l)
		why := ""
		if l == LimitTokens && b.TokensUsed == nil && (s.Running || summaryEnded(s)) {
			why = "no turn has reported completion_tokens_used yet"
		}
		rows = append(rows, limitRow(label, used, ceiling, readout, why, w)...)
	}

	why := ""
	if b.ContextBytes == nil && !noBudget {
		why = "this run's driver has no prompt ceiling: it elides nothing"
	}
	rows = append(rows, limitRow("LAST PROMPT", b.PromptBytes, b.ContextBytes, kB(b.PromptBytes)+"/"+kB(b.ContextBytes), why, w)...)
	return rows
}

// summarySteers is this run's steers, each with its delivery as the
// composer's queue words it: a steer never marks an edit or a check.
func summarySteers(s State, w int) []Row {
	rows := heading("STEERS")
	if len(s.Summary.Steers) == 0 {
		return append(rows, Row{Text: "   no steer was sent this run", Role: RoleLabel})
	}
	for _, q := range s.Summary.Steers {
		r := q.row()
		rows = append(rows, wrapped("   ", "", r.Text, r.Role, w)...)
	}
	return rows
}

// summaryClosing is the model's closing words, fenced: each line made plain
// on its own and drawn behind the fence in the model's role, never a
// check's; or why there are none.
func summaryClosing(s State, w int) []Row {
	rows := heading(summaryLabel)
	c := s.Summary.Closing
	if c.Words == nil {
		return append(rows, wrapped("   ", Dash+"  ", c.Why, RoleLabel, w)...)
	}
	for _, l := range strings.Split(*c.Words, "\n") {
		for _, part := range WrapWords(Plain(l), max(w-5, 1)) {
			rows = append(rows, Row{Spans: []Span{{Text: "   │ ", Role: RoleLabel}, {Text: part}}})
		}
	}
	if c.Cut > 0 {
		rows = append(rows, wrapped("   ", "", fmt.Sprintf("this view cut the last %d of %d bytes", c.Cut, c.Total), RoleLabel, w)...)
	}
	return rows
}

// summaryRows is the level's rows at inner width w.
func summaryRows(s State, w int) []Row {
	rows := summaryEnd(s, w)
	rows = append(rows, summaryChecks(s, w)...)
	rows = append(rows, summaryModes(s, w)...)
	rows = append(rows, summaryRadius(s)...)
	rows = append(rows, summaryFiles(s, w)...)
	rows = append(rows, summaryBudget(s, w)...)
	rows = append(rows, summarySteers(s, w)...)
	return append(rows, summaryClosing(s, w)...)
}

// summary is the level at w by h, from row Nav.Scroll down: its stage on
// the top border, and when its rows do not fit, which are in view on the
// bottom one.
func summary(s State, w, h int) []string {
	rows := summaryRows(s, w-2)
	innerH := max(h-2, 0)
	from := clamp(s.Nav.Scroll, 0, max(len(rows)-innerH, 0))
	bottom := ""
	if len(rows) > innerH {
		to := min(from+innerH, len(rows))
		bottom = fmt.Sprintf("rows %d–%d of %d · %s", from+1, to, len(rows), Keycap("↑")+Keycap("↓")+" "+Keycap("PGUP")+Keycap("PGDN"))
	}
	return Box(s.Theme, summaryTitle(s), bottom, rows[from:], w, h)
}

// SummaryTop is the furthest the summary level scrolls at w by h, the
// view's own limit, as CodeTop is the diff level's (#843).
func SummaryTop(s State, w, h int) int {
	w, h = max(w, MinWidth), max(h, MinHeight)
	_, bodyH := layout(s, w, h)
	return max(len(summaryRows(s, w-railW-2))-max(bodyH-2, 0), 0)
}
