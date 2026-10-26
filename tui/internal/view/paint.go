package view

import (
	"strings"

	"github.com/charmbracelet/lipgloss"

	"github.com/maydaythecoder/trellis/tui/internal/theme"
)

// Painting is the last step of drawing a row: every width is measured, every
// cut made and every pad added on the plain text first, and the colour goes
// round what is left. A cut through painted text could split an escape
// sequence and leave the rest of the screen in its colour.

// Role is what a row is, and so the colour it is drawn in (DESIGN.md
// section 3.1). Each role has one colour, and a colour means one thing.
type Role int

const (
	// RoleText is the model's words and any row with no role of its own:
	// ink. The zero Row is this role.
	RoleText Role = iota
	// RoleYou is the person: what they typed, and what needs them, such as
	// a question waiting or files left for their review: signal.
	RoleYou
	// RoleTool is a tool call, the engine's own activity: cool.
	RoleTool
	// RolePassed is a check trellis reported passing: ok. A row takes it
	// only from the verdict trellis sent, never from its words (ADR 0004).
	RolePassed
	// RoleFailed is a check trellis reported failing: down. As RolePassed,
	// only from a verdict.
	RoleFailed
	// RoleNote is a note to the person, a WARN or an INFO line: warn.
	RoleNote
	// RoleLabel is a title, a caption or a key hint, said about what is
	// drawn rather than drawn: ink-secondary.
	RoleLabel
	// RoleFrame is the box-drawing of frames and rules: line.
	RoleFrame
	// RoleOutput is what a tool returned, under its call: ink-secondary.
	RoleOutput
	// RoleRefusal is an error or a refusal, trellis's or this view's: down.
	// Unlike RoleFailed it says nothing about a check.
	RoleRefusal
	// RoleMeter is a Meter's filled cells up to 70% of its ceiling: cool,
	// the engine's own use of a limit (components.go).
	RoleMeter
	// RoleMeterNear is a Meter's filled cells past 70% of its ceiling:
	// signal, a limit the person set coming near (the Budget card's tones,
	// which DESIGN.md section 2.1 keeps).
	RoleMeterNear
	// RoleMeterOver is a Meter's filled cells past 90% of its ceiling, at
	// it or past it: down.
	RoleMeterOver
	// RoleMeterEmpty is a Meter's empty cells: line-strong.
	RoleMeterEmpty
	// RoleDiffAdd, RoleDiffDel and RoleDiffHunk are a diff's added and
	// removed lines' text and its @@ header (the Diff card, #853):
	// diff-add, diff-del and diff-hunk. They say what a line is in the
	// diff trellis sent, never whether the change is right.
	RoleDiffAdd
	RoleDiffDel
	RoleDiffHunk

	// roles is how many roles there are; every one below it has a colour.
	roles
)

// Span is a run of a row's text in one role. A row drawn in more than one
// colour, such as a Meter's cells and its readout, is a list of them.
type Span struct {
	Text string
	Role Role
}

// Row is one row of a box's inside, or of the transcript: its plain text
// and the role it is painted in.
type Row struct {
	Text string
	Role Role
	// Turn is the turn a transcript row came from, for the rows a turn
	// wrote (its call and what the call returned); 0 for every other row.
	Turn int
	// Spans, when set, are the row in more than one role, drawn in order
	// in place of Text and Role. Only rows the view builds carry them (the
	// budget's Meters); a transcript row is one role.
	Spans []Span
}

// spans is the row as the spans Box draws: its own, or its text in its
// role.
func (r Row) spans() []Span {
	if r.Spans != nil {
		return r.Spans
	}
	return []Span{{Text: r.Text, Role: r.Role}}
}

// rowsOf is lines as rows of one role.
func rowsOf(r Role, lines ...string) []Row {
	out := make([]Row, len(lines))
	for i, l := range lines {
		out[i] = Row{Text: l, Role: r}
	}
	return out
}

// style is the theme's style for role r.
func style(th theme.Theme, r Role) lipgloss.Style {
	switch r {
	case RoleYou:
		return th.Signal
	case RoleTool:
		return th.Cool
	case RolePassed:
		return th.OK
	case RoleFailed:
		return th.Down
	case RoleNote:
		return th.Warn
	case RoleLabel:
		return th.InkSecondary
	case RoleFrame:
		return th.Line
	case RoleOutput:
		return th.InkSecondary
	case RoleRefusal:
		return th.Down
	case RoleMeter:
		return th.Cool
	case RoleMeterNear:
		return th.Signal
	case RoleMeterOver:
		return th.Down
	case RoleMeterEmpty:
		return th.LineStrong
	case RoleDiffAdd:
		return th.DiffAdd
	case RoleDiffDel:
		return th.DiffDel
	case RoleDiffHunk:
		return th.DiffHunk
	default:
		return th.Ink
	}
}

// paint draws s, plain text already cut to its room, in role r's colour.
// Blank text is left as it is: a colour on spaces shows nothing and costs
// two escape sequences. The zero Theme paints nothing, as a terminal with
// no colour draws.
func paint(th theme.Theme, r Role, s string) string {
	if strings.TrimSpace(s) == "" {
		return s
	}
	return style(th, r).Render(s)
}

// fill is s cut to w cells, painted in role r's colour, and padded with
// spaces to exactly w: Pad, painted.
func fill(th theme.Theme, r Role, s string, w int) string {
	return line(th, []Span{{Text: s, Role: r}}, w)
}

// line is a row in more than one role: spans cut to w cells, each piece
// painted in its role, and padded with spaces to exactly w. The cut is
// Cut's, made on the spans' plain text joined, and the "…" that says so
// takes the role of the span it cut. Neighbouring pieces of one role are
// painted as one, so a row of one role draws as fill always has.
func line(th theme.Theme, spans []Span, w int) string {
	whole := spansText(spans)
	kept := Cut(whole, w)
	// Cut keeps a prefix of the text, then "…" when it cut and had room.
	end, tail := len(kept), ""
	if kept != whole && w > 1 {
		tail = "…"
		end -= len(tail)
	}
	var pieces []Span
	add := func(text string, r Role) {
		if text == "" {
			return
		}
		if n := len(pieces); n > 0 && pieces[n-1].Role == r {
			pieces[n-1].Text += text
			return
		}
		pieces = append(pieces, Span{Text: text, Role: r})
	}
	at, last := 0, RoleText
	for _, sp := range spans {
		if at >= end {
			break
		}
		take := min(len(sp.Text), end-at)
		add(sp.Text[:take], sp.Role)
		at, last = at+take, sp.Role
	}
	add(tail, last)
	var out strings.Builder
	for _, p := range pieces {
		out.WriteString(paint(th, p.Role, p.Text))
	}
	return out.String() + strings.Repeat(" ", max(w-Width(kept), 0))
}

// spansWidth is how many cells spans take on one row.
func spansWidth(spans []Span) int { return Width(spansText(spans)) }

// spansText is spans' plain text, joined.
func spansText(spans []Span) string {
	var b strings.Builder
	for _, sp := range spans {
		b.WriteString(sp.Text)
	}
	return b.String()
}
