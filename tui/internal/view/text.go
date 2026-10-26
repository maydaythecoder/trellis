package view

import (
	"iter"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/charmbracelet/lipgloss"
)

// The text helpers every region of the screen and the modal share (#839),
// and the model too for Plain and FirstLine. A width here is terminal
// cells, as Lip Gloss and Bubble Tea's renderer measure a row: 中 and 🙂
// take two cells, a combining accent none. Counted in runes, a line of
// wide characters ran past the screen's edge, and the renderer cut that
// row's right border and the column beside it.

// Plain removes control characters (C0, DEL and C1, ESC among them, and
// line breaks), so served text can never move the cursor, recolour the
// screen or start an escape sequence in the terminal, and the bidirectional
// embeddings, overrides and isolates, so it can never draw in an order
// other than the one it is stored in (a filename or a line of code that
// reads as something else, #803). Everything the model or trellis wrote
// passes through it before it is kept or drawn.
func Plain(s string) string {
	if strings.IndexFunc(s, droppedRune) < 0 {
		return s
	}
	return strings.Map(func(r rune) rune {
		if droppedRune(r) {
			return -1
		}
		return r
	}, s)
}

// droppedRune is a rune Plain drops. The bidirectional controls are format
// characters (Cf), not controls (Cc), so unicode.IsControl misses them:
// U+202A-U+202E (LRE, RLE, PDF, LRO, RLO) and U+2066-U+2069 (LRI, RLI,
// FSI, PDI).
func droppedRune(r rune) bool {
	return unicode.IsControl(r) || (r >= 0x202A && r <= 0x202E) || (r >= 0x2066 && r <= 0x2069)
}

// Width is how many terminal cells s takes on one row, as lipgloss.Width
// measures it. Printable ASCII, most of what is drawn, is one cell a byte
// in that measure and is counted without it: the transcript is wrapped
// again on every frame.
func Width(s string) int {
	for i := 0; i < len(s); i++ {
		if !printableASCII(s[i]) {
			return lipgloss.Width(s)
		}
	}
	return len(s)
}

// Cut keeps s to at most w cells. A cut string ends in "…", so the reader
// knows there was more; at one cell there is no room to say so.
func Cut(s string, w int) string {
	if Width(s) <= w {
		return s
	}
	if w <= 0 {
		return ""
	}
	tail := "…"
	if w == 1 {
		tail = ""
	}
	room := w - Width(tail)
	end, used := 0, 0
	for c := range chars(s) {
		cw := Width(c)
		if used+cw > room {
			break
		}
		end += len(c)
		used += cw
	}
	kept := s[:end]
	// The characters' widths sum to the whole's for every sequence
	// TestTextCharactersSumToTheWhole tries; measuring the whole again
	// keeps at most w cells true for one it does not.
	for kept != "" && Width(kept+tail) > w {
		_, size := utf8.DecodeLastRuneInString(kept)
		kept = kept[:len(kept)-size]
	}
	return kept + tail
}

// CutLeft keeps the end of s to at most w cells, as Cut keeps its start:
// for a line whose end matters most, as the task line's end is where the
// cursor is. A cut string begins with "…"; at one cell there is no room
// to say so.
func CutLeft(s string, w int) string {
	if Width(s) <= w {
		return s
	}
	if w <= 0 {
		return ""
	}
	head := "…"
	if w == 1 {
		head = ""
	}
	room := w - Width(head)
	var cs []string
	for c := range chars(s) {
		cs = append(cs, c)
	}
	start, used := len(s), 0
	for i := len(cs) - 1; i >= 0; i-- {
		cw := Width(cs[i])
		if used+cw > room {
			break
		}
		start -= len(cs[i])
		used += cw
	}
	kept := s[start:]
	// As in Cut: the whole is measured again, so at most w cells holds for
	// a sequence whose characters' widths do not sum to the whole's.
	for kept != "" && Width(head+kept) > w {
		_, size := utf8.DecodeRuneInString(kept)
		kept = kept[size:]
	}
	return head + kept
}

// Pad cuts s to w cells and fills it with spaces to exactly w, so a row
// ends where the next column or the border begins.
func Pad(s string, w int) string {
	s = Cut(s, w)
	return s + strings.Repeat(" ", max(w-Width(s), 0))
}

// WrapWords breaks s into rows of at most w cells at its spaces, as the
// ratatui paragraph did: a row cut inside a word reads as a different word
// (#655). A word wider than a row is broken across rows by WrapHard.
// Runs of spaces inside a row are kept; an empty s is one empty row.
func WrapWords(s string, w int) []string {
	w = max(w, 1)
	if Width(s) <= w {
		return []string{s}
	}
	var out []string
	row, used := "", 0
	for i, word := range strings.Split(s, " ") {
		ww := Width(word)
		if i > 0 {
			if used+1+ww <= w {
				row += " " + word
				used += 1 + ww
				continue
			}
			out = append(out, row)
		}
		if ww > w {
			pieces := WrapHard(word, w)
			out = append(out, pieces[:len(pieces)-1]...)
			word = pieces[len(pieces)-1]
			ww = Width(word)
		}
		row, used = word, ww
	}
	return append(out, row)
}

// WrapHard breaks s into rows of at most w cells wherever the room runs
// out, as a diff is read: broken at a word, a line of code would move its
// columns. An empty s is one empty row.
func WrapHard(s string, w int) []string {
	w = max(w, 1)
	if Width(s) <= w {
		return []string{s}
	}
	var out []string
	start, at, used := 0, 0, 0
	for c := range chars(s) {
		cw := Width(c)
		if used > 0 && used+cw > w {
			out = append(out, s[start:at])
			start, used = at, 0
		}
		at += len(c)
		used += cw
	}
	return append(out, s[start:])
}

// FirstLine is the first line of s that is not blank, or "" when every
// line is: an error's first line is the one that says what went wrong.
func FirstLine(s string) string {
	for _, l := range strings.Split(s, "\n") {
		if strings.TrimSpace(l) != "" {
			return l
		}
	}
	return ""
}

// chars yields s in what a cut or a hard wrap keeps whole: a character
// with every rune that joins it as the renderer measures them, a combining
// accent, a variation selector, a flag's second half, a skin tone, the
// rest of a joined emoji. A rune joins the character before it when it
// takes no cell of its own or adds none to that character; so a cut never
// leaves an accent without its letter, and the characters' widths sum to
// the whole's. In order, they join back into s.
func chars(s string) iter.Seq[string] {
	return func(yield func(string) bool) {
		start := 0
		for i := 0; i < len(s); {
			_, size := utf8.DecodeRuneInString(s[i:])
			next := i + size
			if i > start && startsChar(s, start, i, next) {
				if !yield(s[start:i]) {
					return
				}
				start = i
			}
			i = next
		}
		if start < len(s) {
			yield(s[start:])
		}
	}
}

// startsChar is whether the rune s[i:next] begins a character of its own
// after the one at s[start:i]. Printable ASCII after printable ASCII
// always does, and is most of what is drawn, so it is not measured.
func startsChar(s string, start, i, next int) bool {
	if next == i+1 && printableASCII(s[i]) && printableASCII(s[i-1]) {
		return true
	}
	return Width(s[i:next]) > 0 && Width(s[start:next]) > Width(s[start:i])
}

func printableASCII(b byte) bool { return b >= 0x20 && b < 0x7f }
