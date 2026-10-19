// Package view draws the Go TUI. It holds no state and changes none: each
// function takes what it draws and returns the text, so a test reads the
// screen as a string. It imports nothing of the TUI's own, so the model can
// use it without a cycle.
package view

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// Modal draws the approval question in a bordered box at the centre of a
// width by height screen, and nothing else: what was under it is covered,
// not showing through, so the keys on screen are the ones that act.
//
// The question is on the box's top border and the answer keys on its
// bottom one, and the diff is drawn between them from line scroll (#640).
// The question used to come after the diff, and a long diff pushed it out
// of the box with its own tail, so a change was answered unseen; on the
// border it is drawn whatever the diff's length. When the diff does not
// fit, a row of its own says which lines are in view.
func Modal(question, diff string, scroll, width, height int) string {
	boxW := clamp(width*6/10, 40, width)
	boxH := clamp(height*6/10, 5, height)
	inner, innerH := boxW-2, boxH-2

	// edit and write_file change the tree; anything else the gate asks
	// about (bash, ask_human, a custom tool) is a call.
	what := "call"
	if strings.HasPrefix(question, "edit ") || strings.HasPrefix(question, "write_file ") {
		what = "change"
	}

	lines := strings.Split(diff, "\n")
	rows := func(l string) int { return len(wrap(l, inner)) }
	total := 0
	for _, l := range lines {
		total += rows(l)
	}
	scrolls := total > innerH
	bodyH := innerH
	if scrolls {
		bodyH--
	}
	// The furthest down a view can start and still fill the box, so the
	// last page is a full one and a scroll past it shows that page.
	lastFrom, used := len(lines), 0
	for lastFrom > 0 && used+rows(lines[lastFrom-1]) <= bodyH {
		used += rows(lines[lastFrom-1])
		lastFrom--
	}
	from := scroll
	if from > lastFrom {
		from = lastFrom
	}
	if from < 0 {
		from = 0
	}
	var body []string
	to := from
	for i := from; i < len(lines) && len(body) < bodyH; i++ {
		r := wrap(lines[i], inner)
		if len(body)+len(r) <= bodyH {
			to = i + 1
		}
		body = append(body, r...)
	}
	if len(body) > bodyH {
		body = body[:bodyH]
	}
	for len(body) < bodyH {
		body = append(body, "")
	}
	if scrolls {
		shown := to
		if shown < from+1 {
			shown = from + 1
		}
		body = append(body, fmt.Sprintf("lines %d–%d of %d · ↑↓ PgUp PgDn", from+1, shown, len(lines)))
	}

	out := []string{modalEdge("┌", "┐", question+"?", inner)}
	for _, l := range body {
		out = append(out, "│"+modalPad(cut(l, inner), inner)+"│")
	}
	out = append(out, modalEdge("└", "┘", "[y] accept this "+what+"  [n] reject", inner))
	return lipgloss.Place(width, height, lipgloss.Center, lipgloss.Center, strings.Join(out, "\n"))
}

func clamp(v, lo, hi int) int {
	if v < lo {
		v = lo
	}
	if v > hi {
		v = hi
	}
	return v
}

// modalEdge is a border with a label in it, cut to fit.
func modalEdge(left, right, label string, inner int) string {
	t := cut(" "+label+" ", inner)
	return left + t + strings.Repeat("─", inner-len([]rune(t))) + right
}

func modalPad(s string, w int) string {
	if n := len([]rune(s)); n < w {
		return s + strings.Repeat(" ", w-n)
	}
	return s
}

// cut keeps at most w runes, so a long line is cut rather than wrapped.
func cut(s string, w int) string {
	r := []rune(s)
	if len(r) <= w {
		return s
	}
	if w <= 1 {
		return string(r[:w])
	}
	return string(r[:w-1]) + "…"
}

// wrap breaks a line into rows of at most w runes.
func wrap(s string, w int) []string {
	r := []rune(s)
	if len(r) == 0 {
		return []string{""}
	}
	var out []string
	for len(r) > w {
		out = append(out, string(r[:w]))
		r = r[w:]
	}
	return append(out, string(r))
}
