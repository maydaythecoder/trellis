package view

import (
	"strings"
	"testing"
)

// samples mixes one-cell, two-cell and zero-cell characters, the cases a
// count of runes got wrong.
var samples = []string{
	"",
	"plain ascii text",
	"中文字符和更多的中文字符",
	"🙂 emoji 🙂 between 🙂 words",
	"é combining é marks é",
	"mixed 中文 and ascii 和 emoji 🙂 together",
	"👨‍👩‍👧 a joined emoji, then 🇺🇸 a flag",
}

// A width is cells, as the renderer measures a row: a CJK character or an
// emoji is two, a combining accent none.
func TestTextWidthIsCells(t *testing.T) {
	for s, want := range map[string]int{"abc": 3, "中文": 4, "🙂": 2, "é": 1, "…": 1, "│─┌": 3} {
		if got := Width(s); got != want {
			t.Errorf("Width(%q) = %d, want %d", s, got, want)
		}
	}
}

// Cut keeps at most w cells and ends a cut string in "…"; it never splits
// an accent from its letter, and never runs past w, whatever the text.
func TestTextCutKeepsAtMostTheWidthAndSaysItCut(t *testing.T) {
	for _, c := range []struct {
		s    string
		w    int
		want string
	}{
		{"abc", 3, "abc"},
		{"abcd", 3, "ab…"},
		{"abcd", 1, "a"},
		{"abcd", 0, ""},
		{"中文字符", 5, "中文…"},
		{"中文字符", 4, "中…"},
		{"中文", 1, ""},
		{"ééé", 2, "é…"},
		{"🇺🇸🇬🇧🇫🇷", 5, "🇺🇸🇬🇧…"},
	} {
		if got := Cut(c.s, c.w); got != c.want {
			t.Errorf("Cut(%q, %d) = %q, want %q", c.s, c.w, got, c.want)
		}
	}
	for _, s := range samples {
		for w := 0; w <= Width(s)+1; w++ {
			if got := Cut(s, w); Width(got) > w {
				t.Fatalf("Cut(%q, %d) = %q is %d cells", s, w, got, Width(got))
			}
		}
	}
}

// Pad is exactly w cells whatever it is given, so the border after it is
// in the same column on every row.
func TestTextPadIsExactlyTheWidth(t *testing.T) {
	for _, s := range samples {
		for w := 0; w <= Width(s)+3; w++ {
			if got := Pad(s, w); Width(got) != w {
				t.Fatalf("Pad(%q, %d) = %q is %d cells", s, w, got, Width(got))
			}
		}
	}
}

// WrapWords breaks at spaces, counting cells; a word wider than the row is
// broken across rows; indentation and an empty line are kept.
func TestTextWrapWordsBreaksAtSpacesInCells(t *testing.T) {
	for _, c := range []struct {
		s    string
		w    int
		want []string
	}{
		{"中文 中文 中文", 9, []string{"中文 中文", "中文"}},
		{"中文中文中文", 5, []string{"中文", "中文", "中文"}},
		{"    an indented line", 40, []string{"    an indented line"}},
		{"", 10, []string{""}},
	} {
		if got := WrapWords(c.s, c.w); strings.Join(got, "|") != strings.Join(c.want, "|") {
			t.Errorf("WrapWords(%q, %d) = %q, want %q", c.s, c.w, got, c.want)
		}
	}
	for _, s := range samples {
		for w := 2; w <= 12; w++ {
			for _, row := range WrapWords(s, w) {
				if Width(row) > w {
					t.Fatalf("WrapWords(%q, %d) has a %d-cell row %q", s, w, Width(row), row)
				}
			}
		}
	}
}

// WrapHard breaks wherever the room runs out, counting cells, and an empty
// line is one empty row.
func TestTextWrapHardBreaksAnywhereInCells(t *testing.T) {
	if got := WrapHard("中文abc", 3); strings.Join(got, "|") != "中|文a|bc" {
		t.Fatalf("got %q", got)
	}
	if got := WrapHard("", 3); len(got) != 1 || got[0] != "" {
		t.Fatalf("got %q", got)
	}
	for _, s := range samples {
		for w := 2; w <= 12; w++ {
			rows := WrapHard(s, w)
			if strings.Join(rows, "") != s {
				t.Fatalf("WrapHard(%q, %d) lost text: %q", s, w, rows)
			}
			for _, row := range rows {
				if Width(row) > w {
					t.Fatalf("WrapHard(%q, %d) has a %d-cell row %q", s, w, Width(row), row)
				}
			}
		}
	}
}

// A character keeps what joins it, so the characters join back into the
// text and their widths sum to the whole's: a cut or a wrap never splits a
// flag, a skin tone, a joined emoji, a vowel sign, Hangul jamo or an
// accent from what it belongs to, and never leaves a cell unused for one.
func TestTextCharactersSumToTheWhole(t *testing.T) {
	joined := []string{"🇺🇸🇬🇧", "👍🏽", "☺️", "#️⃣", "कि", "각", "🏳️‍🌈"}
	for _, s := range append(append([]string{}, samples...), joined...) {
		var back strings.Builder
		sum := 0
		for c := range chars(s) {
			back.WriteString(c)
			sum += Width(c)
		}
		if back.String() != s || sum != Width(s) {
			t.Errorf("%q: joins back as %q, widths sum to %d, the whole is %d", s, back.String(), sum, Width(s))
		}
	}
	for s, want := range map[string]int{"🇺🇸": 1, "👍🏽": 1, "👨‍👩‍👧": 1, "é": 1, "ab": 2, "中文": 2} {
		n := 0
		for range chars(s) {
			n++
		}
		if n != want {
			t.Errorf("%q is %d character(s), want %d", s, n, want)
		}
	}
}

// FirstLine is the first line that says something; all blank is "".
func TestTextFirstLineSkipsBlankLines(t *testing.T) {
	if got := FirstLine("\n  \r\nthe reason\nmore"); got != "the reason" {
		t.Fatalf("got %q", got)
	}
	if got := FirstLine(" \n\t\n"); got != "" {
		t.Fatalf("got %q", got)
	}
}

// Plain drops every control character, ESC and the C1 CSI among them, so
// served text cannot start an escape sequence.
func TestTextPlainDropsControlCharacters(t *testing.T) {
	if got := Plain("a\x1b[31mb\x07\r\nc\u009b2J 中"); got != "a[31mbc2J 中" {
		t.Fatalf("got %q", got)
	}
}
