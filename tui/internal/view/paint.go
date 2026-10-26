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
	// RoleYou is what the person typed: signal.
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

	// roles is how many roles there are; every one below it has a colour.
	roles
)

// Row is one row of a box's inside: its plain text and the role it is
// painted in.
type Row struct {
	Text string
	Role Role
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
	s = Cut(s, w)
	return paint(th, r, s) + strings.Repeat(" ", max(w-Width(s), 0))
}
