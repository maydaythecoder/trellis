package view

import (
	"strings"
	"testing"
)

// A box is h rows of exactly w cells whatever its lines hold: wide text is
// cut inside the border, a title too long for the top border is cut and
// says so, and a box with no room for a border is blank rather than
// shorter than its column. The risk: a short row moves the border of
// everything drawn to its right.
func TestFrameBoxIsExactlyItsSize(t *testing.T) {
	const title = "一个很长的标题，比边框还要长"
	lines := []string{"中文字符和更多的中文字符", "🙂🙂🙂🙂🙂🙂🙂🙂", "ééé", "ascii"}
	for _, size := range [][2]int{{12, 6}, {13, 3}, {40, 8}, {1, 3}, {12, 1}, {0, 2}} {
		w, h := size[0], size[1]
		rows := Box(title, "bottom", lines, w, h)
		if len(rows) != h {
			t.Fatalf("%dx%d: %d rows", w, h, len(rows))
		}
		for i, r := range rows {
			if Width(r) != w {
				t.Fatalf("%dx%d: row %d is %d cells: %q", w, h, i, Width(r), r)
			}
		}
		if w < 2 || h < 2 {
			continue
		}
		fits := Width(" "+title+" ") <= w-2
		if !strings.HasPrefix(rows[0], "┌ ") || strings.Contains(rows[0], title) != fits || strings.Contains(rows[0], "…") == fits {
			t.Fatalf("%dx%d: the title is cut only when it does not fit, and then says so: %q", w, h, rows[0])
		}
	}
}
