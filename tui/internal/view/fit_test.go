package view

import (
	"strings"
	"testing"
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
