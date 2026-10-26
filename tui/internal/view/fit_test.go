package view

import (
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"

	"github.com/maydaythecoder/trellis/tui/internal/theme"
	"github.com/maydaythecoder/trellis/tui/internal/theme/themetest"
)

// paneRows is the inside of the pane titled title, as drawn at w by h.
func paneRows(t *testing.T, s State, title string, w, h int) []string {
	t.Helper()
	rows := strings.Split(Render(s, w, h), "\n")
	head := []rune("┌ " + title + " ")
	for y, r := range rows {
		row := []rune(r)
		for x := 0; x+len(head) <= len(row); x++ {
			if string(row[x:x+len(head)]) != string(head) {
				continue
			}
			right := x
			for right < len(row) && row[right] != '┐' {
				right++
			}
			var out []string
			for _, below := range rows[y+1:] {
				b := []rune(below)
				if x >= len(b) || b[x] == '└' {
					break
				}
				out = append(out, string(b[x+1:right]))
			}
			return out
		}
	}
	t.Fatalf("no %q pane:\n%s", title, strings.Join(rows, "\n"))
	return nil
}

// #655: at 80 columns the right column was 32 wide, and the panes read
// "0 ac", "reac" and "a floor on reach, not a boun" where their figures
// are. The column now has a floor, and the labels are whole.
func TestViewFitsEightyColumnsActionsAndRadiusLabelsAreWhole(t *testing.T) {
	s := acted(Action{Tool: "read_file", Target: "app/calc.py"}, Action{Tool: "edit", Target: "app/calc.py", Wrote: true})
	if a := strings.Join(paneRows(t, s, "actions", 80, 24), "\n"); !strings.Contains(a, "2 actions") {
		t.Fatalf("actions:\n%s", a)
	}
	r := strings.Join(paneRows(t, s, "radius", 80, 24), "\n")
	if !strings.Contains(r, "◆ reach ") {
		t.Fatalf("radius:\n%s", r)
	}
	if !strings.Contains(r, "a floor on reach, not a boundary") {
		t.Fatalf("radius:\n%s", r)
	}
	if a := strings.Join(paneRows(t, populated(), "actions", 80, 24), "\n"); !strings.Contains(a, "0 actions") {
		t.Fatalf("actions with nothing called:\n%s", a)
	}
}

// #655: the budget line, cut by its box, ended "· queu" once a turn had
// reported a rate and a wait. Whole segments give way instead, the rate
// first; wider, the same state draws everything.
func TestBudgetLineAtEightyColumnsEndsWithAWholeSegment(t *testing.T) {
	s := populated()
	s.Running = true
	s.Budget = Budget{Iteration: 4, MaxIterations: u(40), PromptBytes: u(16800), ElapsedSecs: u(36),
		CompletionTokens: u(85), ModelMs: u(9600), QueueWaitMs: u(2100)}
	b := strings.Join(paneRows(t, s, "budget", 80, 24), "\n")
	for _, want := range []string{"iteration 4/40", "16800 B prompt", "0:36 elapsed", "queued 2.1 s"} {
		if !strings.Contains(b, want) {
			t.Fatalf("%q missing: %s", want, b)
		}
	}
	if strings.Contains(b, "tok") || strings.Contains(b, "queu ") || strings.Contains(b, "…") {
		t.Fatalf("the rate did not give way first, or a segment was cut: %s", b)
	}
	wide := Render(s, 140, 24)
	if !strings.Contains(wide, "85 tok, 11.3 tok/s") || !strings.Contains(wide, "queued 2.1 s") {
		t.Fatalf("wider, the rate is not drawn:\n%s", wide)
	}
}

// wideState puts text two cells wide per character in every place the
// screen draws served or typed text: the bar's model and tier, the
// transcript (one line a single word wider than its pane), the outcome on
// its border, an edit's path, a check's name, a tool and its target, the
// typed line and the @ paths offered under it.
func wideState() State {
	s := populated()
	s.Model = "通义千问-编码器-三十二B-指令版-量化四位-中文" // a long spec, cut to the room left
	s.Tier = "2, 工具已探测"
	s.Transcript = []string{
		"> 修复 应用/计算.py 中的类型错误，然后运行所有检查",
		"· read_file 应用/计算.py 🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂🙂",
		strings.Repeat("中", 90),
		"combining marks take no cell: " + strings.Repeat("e\u0301", 40),
	}
	s.Outcome = "failed: 无法解析模块 应用/计算.py，类型检查没有通过"
	s.Input = "修复 @应用/计"
	s.Offered = []string{"应用/计算.py", "应用/计算/很长的目录名称/另一个很长的目录名称/模块.py"}
	s.Edits = []Edit{{Path: "应用/计算/很长的目录名称/另一个很长的目录名称/模块.py", Status: Proposed}}
	s.Checks = []Check{{Name: "类型检查：所有的模块和所有的测试文件", Passed: true}}
	s.Actions = []Action{
		{Tool: "read_file", Target: "应用/计算.py"},
		{Tool: "整理导入", Target: "应用/" + strings.Repeat("目录/", 20) + "模块.py", Wrote: true},
	}
	return s
}

// #839: widths were counted in runes, and a character two cells wide
// counts one, so a row of them ran past the screen's edge and the terminal
// lost the right border and the panes beside it (on the rune-counted code
// this state drew a 90-cell bar at 80 columns). Measured in cells, every
// row of the screen and of the approval modal is exactly the screen's
// width: none runs past the edge, and each right border is in the last
// column.
func TestViewWideTextKeepsEveryRowWithinTheWidth(t *testing.T) {
	diff := "--- a/应用/计算.py\n+++ b/应用/计算.py\n@@ -1,3 +1,3 @@\n-def 计算(数值):\n+def 计算(数值: int) -> int:\n" +
		"     return " + strings.Repeat("数值 + ", 40) + "1"
	// Unpainted, and painted at truecolor and at 16 colours, measured with
	// lipgloss.Width, which skips the colours' escape sequences (#841).
	themes := map[string]theme.Theme{
		"unpainted":   {},
		"truecolor":   theme.New(themetest.Renderer(t, "TrueColor", true)),
		"16 on light": theme.New(themetest.Renderer(t, "ANSI", false)),
	}
	for paint, th := range themes {
		for _, size := range [][2]int{{80, 24}, {120, 40}} {
			w, h := size[0], size[1]
			s := wideState()
			s.Theme = th
			screens := map[string]string{
				"render": Render(s, w, h),
				"modal":  Modal(th, "edit 应用/计算.py 把所有的类型错误都修复好并且保留原来的行为", diff, 0, w, h),
			}
			for name, screen := range screens {
				rows := strings.Split(screen, "\n")
				if len(rows) != h {
					t.Fatalf("%s, %s, at %dx%d: %d rows, want %d", name, paint, w, h, len(rows), h)
				}
				for i, r := range rows {
					if got := lipgloss.Width(r); got != w {
						t.Fatalf("%s, %s, at %dx%d: row %d is %d cells, want %d:\n%s", name, paint, w, h, i, got, w, screen)
					}
				}
			}
		}
	}
}

// #641: at launch the line read "iteration 0 · 0 B prompt · 0s elapsed",
// three figures nothing had reported. Each is named as unreported; the
// ceiling is drawn without inventing a count against it; and the time
// elapsed is a clock, 75 s as 1:15.
func TestBudgetLineUnreportedFiguresAreNamedNotDrawnAsZero(t *testing.T) {
	out := Render(State{}, 100, 24)
	for _, bad := range []string{"0 B prompt", "iteration 0", "0s elapsed", "0:00 elapsed"} {
		if strings.Contains(out, bad) {
			t.Fatalf("%q drawn:\n%s", bad, out)
		}
	}
	if !strings.Contains(out, "not reported") || !strings.Contains(out, "no iteration yet") {
		t.Fatalf("unreported figures are not named:\n%s", out)
	}
	s := State{Running: true, Budget: Budget{Iteration: 2, MaxIterations: u(40), ElapsedSecs: u(18)}}
	if out := Render(s, 120, 24); !strings.Contains(out, "prompt not reported") || !strings.Contains(out, "iteration 2/40") {
		t.Fatalf("%s", out)
	}
	s = State{Running: true, Budget: Budget{MaxIterations: u(40), ElapsedSecs: u(75)}}
	if out := Render(s, 120, 24); !strings.Contains(out, "1:15 elapsed") || !strings.Contains(out, "no iteration yet of 40") {
		t.Fatalf("%s", out)
	}
}

// #661: the node that changed the tree was told apart only by colour. Its
// count says "wrote" in words: three calls, one write, as the desktop's
// graph says it; a node that only looked says its count alone; and the
// edge column is still one column, whole at 80 columns.
func TestViewWroteInTextOnTheEditNodeAtEightyColumns(t *testing.T) {
	s := acted(
		Action{Tool: "read_file", Target: "app/calc.py"},
		Action{Tool: "edit", Target: "app/calc.py"},
		Action{Tool: "edit", Target: "app/calc.py"},
		Action{Tool: "edit", Target: "app/calc.py", Wrote: true},
	)
	rows := paneRows(t, s, "actions", 80, 24)
	var edit, read string
	for _, r := range rows {
		switch {
		case strings.HasPrefix(r, "  edit "):
			edit = r
		case strings.HasPrefix(r, "  read_file "):
			read = r
		}
	}
	if !strings.Contains(edit, "3 · 1 wrote") || !strings.Contains(edit, "◆ 4 actions") {
		t.Fatalf("edit node %q in:\n%s", edit, strings.Join(rows, "\n"))
	}
	if read == "" || strings.Contains(read, "wrote") {
		t.Fatalf("read_file node %q claims a write", read)
	}
	cols := map[int]bool{}
	for _, r := range rows {
		for i, c := range []rune(r) {
			if c == '●' || c == '○' {
				cols[i] = true
				break
			}
		}
	}
	if len(cols) != 1 {
		t.Fatalf("the edge column is %d columns:\n%s", len(cols), strings.Join(rows, "\n"))
	}
}

// #875: a tool name of 59 cells or more widened the right column past the
// screen at 80 columns (#839's review). The column stops at the
// transcript's floor and the name is cut in its node: every row is the
// screen's width, the transcript keeps MinTranscriptWidth, and the node's
// count and the edge column are drawn.
func TestViewALongToolNameIsCutNotTheLayout(t *testing.T) {
	for _, n := range []int{20, 59, 120} {
		name := strings.Repeat("x", n)
		s := acted(
			Action{Tool: "read_file", Target: "app/calc.py"},
			Action{Tool: name, Target: "app/calc.py"},
			Action{Tool: name, Target: "app/calc.py", Wrote: true},
		)
		// Tall enough that the actions pane shows every node.
		for _, size := range [][2]int{{80, 40}, {120, 40}} {
			w, h := size[0], size[1]
			screen := Render(s, w, h)
			for i, r := range strings.Split(screen, "\n") {
				if got := Width(r); got != w {
					t.Fatalf("name of %d at %dx%d: row %d is %d cells, want %d:\n%s", n, w, h, i, got, w, screen)
				}
			}
			if left, _ := columns(s, w); left < MinTranscriptWidth {
				t.Fatalf("name of %d at %d columns: the transcript is %d cells, under %d", n, w, left, MinTranscriptWidth)
			}
			rows := paneRows(t, s, "actions", w, h)
			var node string
			cols := map[int]bool{}
			for _, r := range rows {
				if strings.HasPrefix(r, "  xx") && node == "" {
					node = r
				}
				for i, c := range []rune(r) {
					if c == '●' || c == '○' {
						cols[i] = true
						break
					}
				}
			}
			if !strings.Contains(node, "2 · 1 wrote ●") {
				t.Fatalf("name of %d at %d columns: node %q lost its count or edge:\n%s", n, w, node, strings.Join(rows, "\n"))
			}
			// Only the 20-cell name at 120 columns fits beside its count
			// and the focus; every other is cut, and says so.
			want := n == 20 && w == 120
			if whole := strings.Contains(node, name+" "); whole != want || (!whole && !strings.Contains(node, "…")) {
				t.Fatalf("name of %d at %d columns: node %q, whole %v, want %v", n, w, node, whole, want)
			}
			if len(cols) != 1 {
				t.Fatalf("name of %d at %d columns: the edge column is %d columns:\n%s", n, w, len(cols), strings.Join(rows, "\n"))
			}
		}
	}
}
