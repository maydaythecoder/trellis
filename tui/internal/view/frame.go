package view

import (
	"strings"

	"github.com/maydaythecoder/trellis/tui/internal/theme"
)

// Box draws rows in a w by h frame, with title in the top border and bottom
// in the bottom one. Every pane, the budget, the task entry and the
// approval modal are drawn with it (#839). A row is cut to the inside,
// never wrapped, so a long one cannot push the right border out, and a box
// too small for its rows keeps the first. It returns h rows, each exactly
// w cells, so frames set side by side stay square; a box with no room for
// its border is blank.
//
// The frame is drawn in the line colour, the title and the bottom as
// labels, and each row in its role's colour, from th (#841).
func Box(th theme.Theme, title, bottom string, rows []Row, w, h int) []string {
	if h < 2 || w < 2 {
		out := make([]string, max(h, 0))
		for i := range out {
			out[i] = strings.Repeat(" ", max(w, 0))
		}
		return out
	}
	inner := w - 2
	side := paint(th, RoleFrame, "│")
	out := []string{edge(th, "┌", "┐", title, inner)}
	for i := 0; i < h-2; i++ {
		var r Row
		if i < len(rows) {
			r = rows[i]
		}
		out = append(out, side+fill(th, r.Role, r.Text, inner)+side)
	}
	return append(out, edge(th, "└", "┘", bottom, inner))
}

// edge is a top or bottom border with a label in it, cut to the inside.
// The label is made plain here: it is often served text, such as a run's
// outcome or an approval's question.
func edge(th theme.Theme, left, right, label string, inner int) string {
	if label == "" {
		return paint(th, RoleFrame, left+strings.Repeat("─", inner)+right)
	}
	t := Cut(" "+Plain(label)+" ", inner)
	return paint(th, RoleFrame, left) + paint(th, RoleLabel, t) + paint(th, RoleFrame, strings.Repeat("─", inner-Width(t))+right)
}
