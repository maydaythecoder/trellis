package view

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"

	"github.com/maydaythecoder/trellis/tui/internal/theme"
)

// modalBox is the modal's size at width by height, and its inside width.
func modalBox(width, height int) (boxW, boxH, inner int) {
	boxW = clamp(width*6/10, 40, width)
	boxH = clamp(height*6/10, 5, height)
	return boxW, boxH, boxW - 2
}

// modalPage is the diff's lines, the rows the body shows, whether the diff
// scrolls (a row then says which lines are in view), and lastFrom: the
// furthest down a view can start and still fill the box, so the last page
// is a full one. A last line taller than the box is its own last page.
func modalPage(diff string, width, height int) (lines []string, bodyH int, scrolls bool, lastFrom int) {
	_, boxH, inner := modalBox(width, height)
	innerH := boxH - 2
	// Each line is made plain here as well as by the model: the diff is
	// served text, and the view draws no character it did not check.
	lines = strings.Split(diff, "\n")
	for i, l := range lines {
		lines[i] = Plain(l)
	}
	rows := func(l string) int { return len(WrapHard(l, inner)) }
	total := 0
	for _, l := range lines {
		total += rows(l)
	}
	scrolls = total > innerH
	bodyH = innerH
	if scrolls {
		bodyH--
	}
	lastFrom, used := len(lines), 0
	for lastFrom > 0 && used+rows(lines[lastFrom-1]) <= bodyH {
		used += rows(lines[lastFrom-1])
		lastFrom--
	}
	// When the last line alone wraps past the box, nothing fits from the
	// end and the loop stops at len(lines), a start past every line: one
	// Down on a long bash command emptied the box and read "lines 2–2 of
	// 1". The view starts at that line instead and shows what fits of it.
	lastFrom = min(lastFrom, len(lines)-1)
	return lines, bodyH, scrolls, lastFrom
}

// ModalLastFrom is the furthest line the modal's view can start from at
// width by height and still fill its box: the most the model scrolls to.
// The model stopped at the diff's last line while the view stopped here,
// so the first Up after the end moved nothing (#843).
func ModalLastFrom(diff string, width, height int) int {
	_, _, _, lastFrom := modalPage(diff, width, height)
	return lastFrom
}

// Modal draws the approval question in a bordered box at the centre of a
// width by height screen, and nothing else: what was under it is covered,
// not showing through, so the keys on screen are the ones that act.
//
// The question is on the box's top border and the answer keys on its
// bottom one, and the diff is drawn between them from line scroll (#640).
// The question used to come after the diff, and a long diff pushed it out
// of the box with its own tail, so a change was answered unseen; on the
// border it is drawn whatever the diff's length. When the diff does not
// fit, a row of its own says which lines are in view. th paints it as Box
// paints a pane; the diff's lines are text until the code level colours
// them by line role (#853).
func Modal(th theme.Theme, question, diff string, scroll, width, height int) string {
	boxW, boxH, inner := modalBox(width, height)

	// edit and write_file change the tree; anything else the gate asks
	// about (bash, ask_human, a custom tool) is a call.
	what := "call"
	if strings.HasPrefix(question, "edit ") || strings.HasPrefix(question, "write_file ") {
		what = "change"
	}

	lines, bodyH, scrolls, lastFrom := modalPage(diff, width, height)
	from := scroll
	if from > lastFrom {
		from = lastFrom
	}
	if from < 0 {
		from = 0
	}
	var body []Row
	to := from
	for i := from; i < len(lines) && len(body) < bodyH; i++ {
		r := WrapHard(lines[i], inner)
		if len(body)+len(r) <= bodyH {
			to = i + 1
		}
		body = append(body, rowsOf(RoleText, r...)...)
	}
	if len(body) > bodyH {
		body = body[:bodyH]
	}
	for len(body) < bodyH {
		body = append(body, Row{})
	}
	if scrolls {
		shown := to
		if shown < from+1 {
			shown = from + 1
		}
		scrollKeys := Keycap("↑") + Keycap("↓") + " " + Keycap("PGUP") + Keycap("PGDN")
		body = append(body, Row{Text: fmt.Sprintf("lines %d–%d of %d · %s", from+1, shown, len(lines), scrollKeys), Role: RoleLabel})
	}

	out := Box(th, question+"?", keyRow(key{"y", "accept this " + what}, key{"n", "reject"}), body, boxW, boxH)
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
