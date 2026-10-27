package view

import (
	"fmt"
	"strings"
)

// The action log's filters, quiet mode and the raw call (#948), the
// desktop's #949 for the TUI. A filter and quiet mode decide which of the
// transcript's rows are drawn; neither takes a row out of it, so switching
// back draws every row still held under the cap. The scroll limits count
// only the rows drawn (shownRows), as the pane does, or a filter that hid
// most rows left Home and PgUp spending presses on rows not there (#843).

// LogFilter is which rows the transcript draws, the desktop's filter bar's
// groups (index.html's log-filters, style.css's data-kind rules).
type LogFilter string

// The filters, in the order ^F visits them. FilterAll is the zero value's
// meaning too, so a State built without one draws every row.
const (
	FilterAll    LogFilter = "all"
	FilterYou    LogFilter = "you"
	FilterTools  LogFilter = "tools"
	FilterChecks LogFilter = "checks"
	FilterNotes  LogFilter = "notes"
)

// LogFilters is every filter in the order ^F visits them.
var LogFilters = []LogFilter{FilterAll, FilterYou, FilterTools, FilterChecks, FilterNotes}

// Shows reports whether the transcript draws row r under filter f and,
// when quiet, quiet mode. The groups are the desktop's, by the role the
// row's writer gave it and never by its words (model/rows.go):
//
//   - a turn's divider (RoleLabel) under every filter and in quiet mode,
//     as the desktop's turn rule is, so a filtered row still reads under
//     its turn;
//   - You: the person's own rows and the model's question to them;
//   - Tools: the calls, what they returned, and what was refused;
//   - Checks: trellis-checks' verdicts;
//   - Notes: this view's and trellis's notes, refusals, and the model's
//     question.
//
// The model's own words and this view's answers to a /command (RoleText)
// are drawn under All only: the desktop's log carries no such rows, and
// left out its Model filter.
//
// Quiet hides every call that wrote nothing and every observation line:
// the desktop's quiet hides the tool rows that wrote nothing, and its log
// shows no observation until the row is opened, so a quiet TUI log draws
// what a quiet desktop log does. A call that wrote stays, as the trail and
// the Edits pane count it (#800).
func Shows(r Row, f LogFilter, quiet bool) bool {
	if r.Role == RoleLabel {
		return true
	}
	if quiet && (r.Role == RoleOutput || (r.Role == RoleTool && !r.Wrote)) {
		return false
	}
	switch f {
	case FilterYou:
		return r.Role == RoleYou
	case FilterTools:
		return r.Role == RoleTool || r.Role == RoleOutput || r.Role == RoleRefusal
	case FilterChecks:
		return r.Role == RolePassed || r.Role == RoleFailed
	case FilterNotes:
		return r.Role == RoleNote || r.Role == RoleRefusal || r.Ask
	}
	return true
}

// shownRows is the transcript's rows the filter and quiet mode draw, in
// order. TranscriptRows, transcript and TranscriptCall all read through
// it, so the scroll limits and the call ^O opens are taken from the rows
// drawn.
func shownRows(s State) []Row {
	if (s.Filter == "" || s.Filter == FilterAll) && !s.Quiet {
		return s.Transcript
	}
	out := make([]Row, 0, len(s.Transcript))
	for _, r := range s.Transcript {
		if Shows(r, s.Filter, s.Quiet) {
			out = append(out, r)
		}
	}
	return out
}

// logTitle is the transcript's title: the filter and quiet mode when
// either hides a row, with how many it hides, so a log drawn short never
// reads as a log with nothing more in it. The keys are the status bar's.
func logTitle(s State) string {
	title := "transcript"
	if s.Filter != "" && s.Filter != FilterAll {
		title += " · " + string(s.Filter)
	}
	if s.Quiet {
		title += " · quiet"
	}
	if hidden := len(s.Transcript) - len(shownRows(s)); hidden > 0 {
		title += fmt.Sprintf(" · %d hidden", hidden)
	}
	return title
}

// RawCall is a turn's call as the model wrote it and what it returned in
// full, as the turn event carried them, for ^O. The model keeps it on the
// rows the event wrote, so it goes when they leave the cap.
type RawCall struct {
	Turn int
	Call string
	// Observation is nil when the event carried none.
	Observation *string
}

// pageRows is the transcript's rows drawn in a pane of w by h cells, each
// line with the row it was wrapped from, from the first drawn to the last.
func pageRows(s State, w, h int) (lines []Row, from []int) {
	inner := w - 2
	rows := shownRows(s)
	for i, r := range rows {
		for _, l := range rowsOf(r.Role, transcriptLines(r, inner)...) {
			lines = append(lines, l)
			from = append(from, i)
		}
	}
	// A pane too short for an inside shows no rows: h-2 below zero made
	// start pass end and the slice panicked (#842).
	innerH := max(h-2, 0)
	// Scrolled back past the first row, the first page is shown: the view
	// stops at the top rather than drawing an empty pane.
	end := max(len(lines)-s.ScrollBack, min(innerH, len(lines)))
	start := max(end-innerH, 0)
	return lines[start:end], from[start:end]
}

// TranscriptCall is the call ^O opens at w by h: the newest turn's whose
// rows the transcript pane draws now, under its filter, quiet mode and
// scroll, the turn the person is looking at; false when the pane draws
// no row a turn's call wrote. It is found by row, not by turn number: the
// audit's call shares its turn's number and is a call of its own.
func TranscriptCall(s State, w, h int) (*RawCall, bool) {
	w, h = max(w, MinWidth), max(h, MinHeight)
	_, bodyH := layout(s, w, h)
	if folded(w, h) {
		bodyH--
	}
	rows := shownRows(s)
	_, from := pageRows(s, transcriptW(s, w, h), bodyH)
	for i := len(from) - 1; i >= 0; i-- {
		if raw := rows[from[i]].Raw; raw != nil {
			return raw, true
		}
	}
	return nil, false
}

// HasCall reports whether any row the transcript draws carries a call ^O
// could open, for the status bar's keys.
func HasCall(s State) bool {
	rows := shownRows(s)
	for i := len(rows) - 1; i >= 0; i-- {
		if rows[i].Raw != nil {
			return true
		}
	}
	return false
}

// callText is what the call box draws: the call as the model wrote it,
// then what it returned, each line made plain by modalPage.
func callText(r RawCall) string {
	var b strings.Builder
	b.WriteString("the call, as the model wrote it:\n")
	b.WriteString(r.Call)
	if r.Observation == nil {
		b.WriteString("\n\ntrellis sent no observation for it")
		return b.String()
	}
	b.WriteString("\n\nwhat it returned:\n")
	b.WriteString(*r.Observation)
	return b.String()
}

// callTitle is the call box's top border.
func callTitle(r RawCall) string {
	return fmt.Sprintf("turn %d · its call and what it returned", r.Turn)
}

// CallView draws the raw call in the approval box's place, under the
// Trail and the Now strip and over the status bar, scrolled from line
// scroll. It is text only: nothing in it is a verdict or a question.
func CallView(s State, r RawCall, scroll, w, h int) string {
	bottom := keyRow(key{"ESC", "close"})
	if !fits(w, h) {
		return modalWith(s.Theme, callTitle(r), bottom, callText(r), scroll, w, h)
	}
	top := line(s.Theme, trail(s, w), w) + "\n" + line(s.Theme, nowStrip(s, w), w)
	return top + "\n" + modalWith(s.Theme, callTitle(r), bottom, callText(r), scroll, w, h-askingRows) + "\n" + line(s.Theme, statusBar(s, w), w)
}

// CallLastFrom is the furthest line the call box scrolls to at w by h.
func CallLastFrom(r RawCall, w, h int) int {
	return lastFromAround(callText(r), w, h, askingRows)
}
