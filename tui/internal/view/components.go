package view

import (
	"fmt"
	"strings"
)

// The design system's components as the terminal draws them (#911;
// docs/design/system/components/ and faces.md). Every region draws them
// through the helpers here, so a Meter, a Status or a key reads the same
// wherever it stands:
//
//   - a Meter is a fixed number of cells, █ filled and ░ empty, then its two
//     figures, used/ceiling; never a percentage (DESIGN.md section 2, rule 4);
//   - a Status is a dot and its word in one tone, never a dot alone;
//   - a Keycap is the key in brackets, [TAB], with what it does after it;
//   - a figure trellis did not report is Dash, never 0 (rule 2), and /help
//     says what Dash means.

// Dash is a figure trellis did not report.
const Dash = "—"

// The Meter card's cell counts: 8 where a Meter shares its row, 16 where it
// leads one.
const (
	meterCells      = 16
	meterCellsShort = 8
)

// meter is used against its ceiling as a Meter: an uppercase label when
// one is given, the cells, and the readout used/ceiling in ink. A figure
// not reported is Dash in the readout, and with either unreported no cells
// are drawn: cells with no share to show would read as 0.
func meter(label string, used, ceiling *uint64, cells int) []Span {
	var out []Span
	if label != "" {
		out = append(out, Span{Text: label + " ", Role: RoleLabel})
	}
	if used != nil && ceiling != nil {
		on := meterFilled(*used, *ceiling, cells)
		out = append(out,
			Span{Text: strings.Repeat("█", on), Role: meterRole(*used, *ceiling)},
			Span{Text: strings.Repeat("░", cells-on), Role: RoleMeterEmpty},
			Span{Text: " "})
	}
	return append(out, Span{Text: figure(used) + "/" + figure(ceiling)})
}

// meterFilled is how many of cells (two or more) a Meter fills for used of
// ceiling: the share rounded down, except that a figure above 0 fills at
// least one cell, so an empty Meter is exactly 0, and only a figure at its
// ceiling or past it fills every cell.
func meterFilled(used, ceiling uint64, cells int) int {
	if used >= ceiling {
		return cells
	}
	// used < ceiling here, so the share rounded down is under cells.
	n := int(used * uint64(cells) / ceiling)
	if used > 0 && n == 0 {
		n = 1
	}
	return n
}

// meterRole is the tone of a Meter's filled cells: cool, signal past 70%
// of its ceiling and down past 90%, the Budget card's tones.
func meterRole(used, ceiling uint64) Role {
	switch {
	case used*10 > ceiling*9:
		return RoleMeterOver
	case used*10 > ceiling*7:
		return RoleMeterNear
	}
	return RoleMeter
}

// figure is n as a figure, or Dash when trellis did not report it.
func figure(n *uint64) string {
	if n == nil {
		return Dash
	}
	return fmt.Sprint(*n)
}

// statusSpan is a Status: a dot and its word in one tone, never a dot alone
// (the Status card).
func statusSpan(word string, tone Role) Span {
	return Span{Text: "● " + word, Role: tone}
}

// Keycap is a key as the terminal draws a Keycap: in brackets. Keys are
// named as the Keycap card names them (TAB, ESC, ⏎, ^C); a letter typed to
// answer a question, y or n, is drawn as it is typed, since the approval
// box takes the lower case only.
func Keycap(key string) string { return "[" + key + "]" }

// key is a keycap and what it does.
type key struct{ cap, does string }

// keyRow is keys side by side, each as its keycap and what it does:
// "[TAB] select  [⏎] in".
func keyRow(keys ...key) string {
	parts := make([]string, len(keys))
	for i, k := range keys {
		parts[i] = Keycap(k.cap) + " " + k.does
	}
	return strings.Join(parts, "  ")
}
