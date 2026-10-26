package view

import "strings"

// Box draws lines in a w by h frame, with title in the top border and
// bottom in the bottom one. Every pane, the budget, the task entry and the
// approval modal are drawn with it (#839). A line is cut to the inside,
// never wrapped, so a long one cannot push the right border out, and a box
// too small for its lines keeps the first. It returns h rows, each exactly
// w cells, so frames set side by side stay square; a box with no room for
// its border is blank.
func Box(title, bottom string, lines []string, w, h int) []string {
	if h < 2 || w < 2 {
		out := make([]string, max(h, 0))
		for i := range out {
			out[i] = strings.Repeat(" ", max(w, 0))
		}
		return out
	}
	inner := w - 2
	out := []string{edge("┌", "┐", title, inner)}
	for i := 0; i < h-2; i++ {
		l := ""
		if i < len(lines) {
			l = lines[i]
		}
		out = append(out, "│"+Pad(l, inner)+"│")
	}
	return append(out, edge("└", "┘", bottom, inner))
}

// edge is a top or bottom border with a label in it, cut to the inside.
// The label is made plain here: it is often served text, such as a run's
// outcome or an approval's question.
func edge(left, right, label string, inner int) string {
	if label == "" {
		return left + strings.Repeat("─", inner) + right
	}
	t := Cut(" "+Plain(label)+" ", inner)
	return left + t + strings.Repeat("─", inner-Width(t)) + right
}
