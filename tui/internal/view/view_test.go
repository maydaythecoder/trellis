package view

import (
	"fmt"
	"strings"
	"testing"
	"unicode"

	"github.com/maydaythecoder/trellis/tui/internal/theme"
	"github.com/maydaythecoder/trellis/tui/internal/theme/themetest"
)

func u(n uint64) *uint64 { return &n }

// populated is the ratatui view tests' populated(): a line, a pending
// edit, a passing check, a radius and a budget two turns in.
func populated() State {
	return State{
		Transcript: rowsOf(RoleText, "Reading blast radius"),
		Edits:      []Edit{{Path: "src/index/resolve.rs", Status: Proposed}},
		Checks:     []Check{{Name: "typecheck", Passed: true}},
		Radius:     &Radius{Resolved: 47, Candidates: 3, Unresolved: 2},
		Budget:     Budget{Iteration: 2, MaxIterations: u(40), PromptBytes: u(8400), ElapsedSecs: u(18)},
	}
}

func afterTurn(tokens, modelMs, queueMs *uint64) string {
	s := populated()
	s.Budget = Budget{Iteration: 3, MaxIterations: u(40), PromptBytes: u(8400), ElapsedSecs: u(40),
		CompletionTokens: tokens, ModelMs: modelMs, QueueWaitMs: queueMs}
	return Render(s, 140, 24)
}

// #257's rate is output tokens over the model's time less the queue wait:
// 30 tokens in 25 s with 10 s queued is 2.0 tok/s, and the wait is drawn.
func TestBudgetLineRateExcludesQueueWait(t *testing.T) {
	out := afterTurn(u(30), u(25000), u(10000))
	if !strings.Contains(out, "30 tok, 2.0 tok/s") || !strings.Contains(out, "queued 10.0 s") {
		t.Fatalf("%s", out)
	}
}

// An unknown queue wait counts as 0 for the rate and is not drawn.
func TestBudgetLineUnknownQueueWaitIsZeroAndNotDrawn(t *testing.T) {
	out := afterTurn(u(30), u(20000), nil)
	if !strings.Contains(out, "30 tok, 1.5 tok/s") || strings.Contains(out, "queued") {
		t.Fatalf("%s", out)
	}
}

// Without reported tokens there is no rate and no wait; the rest stays.
func TestBudgetLineWithoutTokensDrawsNoRateAndNoWait(t *testing.T) {
	out := afterTurn(nil, u(25000), u(10000))
	if strings.Contains(out, "tok") || strings.Contains(out, "queued") || !strings.Contains(out, "iteration 3/40") {
		t.Fatalf("%s", out)
	}
}

// With no model time left after the wait there is no rate, and never an
// infinite one.
func TestBudgetLineWithNoModelTimeLeftDrawsNoRate(t *testing.T) {
	for _, c := range [][2]*uint64{{u(0), nil}, {u(10000), u(10000)}, {u(5000), u(9000)}, {nil, nil}} {
		out := afterTurn(u(30), c[0], c[1])
		if strings.Contains(out, "tok/s") || strings.Contains(out, "Inf") || strings.Contains(out, "NaN") {
			t.Fatalf("%v: %s", c, out)
		}
	}
}

// A figure trellis did not report is named as unreported, never drawn as
// 0 (#641); the iteration is against the run's own ceiling once it is
// known, and the elapsed time is a clock.
func TestBudgetLineNamesWhatWasNotReported(t *testing.T) {
	s := State{Running: true, Budget: Budget{Iteration: 1}}
	if got := BudgetLine(s); got != "iteration 1  ·  prompt not reported  ·  not started" {
		t.Fatalf("got %q", got)
	}
	s.Budget = Budget{Iteration: 2, MaxIterations: u(40), PromptBytes: u(8400), ElapsedSecs: u(18)}
	if got := BudgetLine(s); got != "iteration 2/40  ·  8400 B prompt  ·  0:18 elapsed" {
		t.Fatalf("got %q", got)
	}
}

// The six regions are on screen at 80x24, with the budget against its own
// ceiling, prompt bytes as bytes, and the elapsed time.
func TestViewSixRegionsAtEightyByTwentyFour(t *testing.T) {
	out := Render(populated(), 80, 24)
	for _, want := range []string{"TRELLIS", "transcript", "edits", "checks", "radius", "budget",
		"Reading blast radius", "resolved", "8400 B prompt", "iteration 2/40", "0:18 elapsed"} {
		if !strings.Contains(out, want) {
			t.Errorf("%q missing from:\n%s", want, out)
		}
	}
	lines := strings.Split(out, "\n")
	if len(lines) != 24 {
		t.Fatalf("%d rows, want 24", len(lines))
	}
	for i, l := range lines {
		if n := Width(l); n > 80 {
			t.Fatalf("row %d is %d cells wide:\n%s", i, n, l)
		}
	}
}

// It lays out again at a larger size, with the panes' contents.
func TestViewLaysOutAgainAtALargerSize(t *testing.T) {
	out := Render(populated(), 120, 40)
	for _, want := range []string{"transcript", "src/index/resolve.rs", "typecheck passed"} {
		if !strings.Contains(out, want) {
			t.Errorf("%q missing:\n%s", want, out)
		}
	}
	if n := len(strings.Split(out, "\n")); n != 40 {
		t.Fatalf("%d rows", n)
	}
}

// Below 80x24 it says so instead of laying out.
func TestViewTooSmallSaysSoInsteadOfLayingOut(t *testing.T) {
	out := Render(populated(), 40, 10)
	if !strings.Contains(out, "too small") || strings.Contains(out, "transcript") {
		t.Fatalf("%s", out)
	}
	if out := Render(populated(), 79, 30); !strings.Contains(out, "need at least 80x24") {
		t.Fatalf("%s", out)
	}
}

func long() State {
	s := State{Running: true}
	for i := 0; i < 200; i++ {
		s.Transcript = append(s.Transcript, Row{Text: fmt.Sprintf("step %d of the run", i)})
	}
	s.Transcript = append(s.Transcript, Row{Text: "THE LAST LINE"})
	return s
}

// The transcript follows its end; scrolled back it stops following and its
// border says how to return; the last run's outcome stays on screen on the
// border wherever the view is.
func TestViewTranscriptFollowsScrollsBackAndKeepsTheOutcome(t *testing.T) {
	out := Render(long(), 100, 30)
	if !strings.Contains(out, "THE LAST LINE") || strings.Contains(out, "step 0 of the run") {
		t.Fatalf("not following:\n%s", out)
	}
	s := long()
	s.ScrollBack = 40
	out = Render(s, 100, 30)
	if strings.Contains(out, "THE LAST LINE") || !strings.Contains(out, "scrolled back") {
		t.Fatalf("scrolled back:\n%s", out)
	}
	s.Running = false
	s.Outcome = "did not converge; worktree restored: true"
	s.Budget = Budget{Iteration: 3, MaxIterations: u(6), PromptBytes: u(2435), ElapsedSecs: u(42)}
	out = Render(s, 120, 30)
	if !strings.Contains(out, "last run: did not converge; worktree restored: true") ||
		!strings.Contains(out, "finished · iteration 3/6") {
		t.Fatalf("outcome:\n%s", out)
	}
}

// The task line shows what is typed with a block cursor, the key that
// sends it and the one that leaves.
func TestViewTaskLineShowsWhatIsTypedAndHowToSendIt(t *testing.T) {
	s := populated()
	s.Input = "fix the type error"
	out := Render(s, 80, 24)
	for _, want := range []string{"fix the type error█", "enter to run", "esc"} {
		if !strings.Contains(out, want) {
			t.Fatalf("%q missing:\n%s", want, out)
		}
	}
}

// #842: sixteen @ completions at 80x24 left the body one row, and the
// transcript sliced past its own end: `slice bounds out of range [1:0]`.
// However many are offered, at every size, the screen is laid out whole:
// h rows, none wider than w, the radius pane's counts drawn, and a list
// that does not fit says how many it leaves out.
func TestViewManyCompletionsNeverPanic(t *testing.T) {
	for _, size := range [][2]int{{80, 24}, {80, 25}, {100, 30}, {120, 40}} {
		w, h := size[0], size[1]
		room := completionRoom(populated(), h)
		for n := 0; n <= 40; n++ {
			s := populated()
			for i := 0; i < n; i++ {
				s.Offered = append(s.Offered, fmt.Sprintf("src/file_%02d.py", i))
			}
			out := Render(s, w, h)
			lines := strings.Split(out, "\n")
			if len(lines) != h {
				t.Fatalf("%dx%d, %d offered: %d rows:\n%s", w, h, n, len(lines), out)
			}
			for i, l := range lines {
				if Width(l) > w {
					t.Fatalf("%dx%d, %d offered: row %d is %d cells:\n%s", w, h, n, i, Width(l), out)
				}
			}
			if !strings.Contains(out, "resolved") || !strings.Contains(out, "a floor on reach") {
				t.Fatalf("%dx%d, %d offered: the radius pane was cut:\n%s", w, h, n, out)
			}
			more := fmt.Sprintf("… %d more", n-(room-1))
			if n > room && !strings.Contains(out, more) {
				t.Fatalf("%dx%d, %d offered: %q missing:\n%s", w, h, n, more, out)
			}
			if n <= room && strings.Contains(out, " more") {
				t.Fatalf("%dx%d, %d offered fit, yet a row says more:\n%s", w, h, n, out)
			}
			if n > 0 && !strings.Contains(out, "tab src/file_00.py") {
				t.Fatalf("%dx%d, %d offered: the first completion is not drawn:\n%s", w, h, n, out)
			}
		}
	}
}

// #842: once the typed line was wider than its box, its end and the
// cursor were cut off, so what was being typed could not be seen. The
// line now keeps its end in view and marks what scrolled off the left.
func TestViewLongInputKeepsTheCursorVisible(t *testing.T) {
	for _, w := range []int{80, 120} {
		s := populated()
		s.Input = strings.Repeat("refactor the resolver ", 12) + "and the end is here"
		out := Render(s, w, 24)
		var row string
		for _, l := range strings.Split(out, "\n") {
			if strings.Contains(l, "█") {
				row = l
			}
		}
		if row == "" {
			t.Fatalf("%d columns: no row holds the cursor:\n%s", w, out)
		}
		if !strings.Contains(row, "and the end is here█") {
			t.Fatalf("%d columns: the end and the cursor are not in view: %q", w, row)
		}
		if !strings.Contains(row, "│…") {
			t.Fatalf("%d columns: what scrolled off the left is not marked: %q", w, row)
		}
		if Width(row) != w {
			t.Fatalf("%d columns: the task row is %d cells: %q", w, Width(row), row)
		}
	}
	s := populated()
	s.Input = "short"
	if out := Render(s, 80, 24); !strings.Contains(out, "│short█") {
		t.Fatalf("a line that fits is drawn whole from its start:\n%s", out)
	}
}

func acted(as ...Action) State {
	s := populated()
	s.Actions = as
	return s
}

// Every tool is a node, a ring, before any is called, and the pane says
// nothing has happened rather than leaving it blank.
func TestViewEveryToolIsANodeBeforeAnyIsCalled(t *testing.T) {
	out := Render(populated(), 100, 40)
	for _, tool := range Tools {
		if !strings.Contains(out, tool) {
			t.Fatalf("%s missing:\n%s", tool, out)
		}
	}
	if !strings.Contains(out, "○") || !strings.Contains(out, "nothing yet") {
		t.Fatalf("%s", out)
	}
}

// A refused edit is on the graph as an action and not counted as a write;
// a run that only looked says so; one action is not "1 actions".
func TestViewActionsCountWritesNotRefusals(t *testing.T) {
	out := Render(acted(Action{"edit", "app/calc.py", false}, Action{"edit", "app/calc.py", false}, Action{"edit", "app/calc.py", true}), 100, 40)
	if !strings.Contains(out, "3 actions") || !strings.Contains(out, "1 wrote to the tree") {
		t.Fatalf("%s", out)
	}
	out = Render(acted(Action{"read_file", "app/calc.py", false}), 100, 40)
	if !strings.Contains(out, "1 action") || strings.Contains(out, "1 actions") || !strings.Contains(out, "nothing written to the tree") {
		t.Fatalf("%s", out)
	}
}

// A tool the view has never heard of still reaches the graph, and the
// edges meet at one focus per figure: one for actions, one for radius.
func TestViewUnknownToolAndOneFocusPerFigure(t *testing.T) {
	out := Render(acted(Action{"summon_kraken", "the deep", false}), 100, 40)
	if !strings.Contains(out, "summon_kraken") {
		t.Fatalf("%s", out)
	}
	out = Render(acted(Action{"edit", "a.py", true}), 100, 40)
	if n := strings.Count(out, "◆"); n != 2 {
		t.Fatalf("%d foci:\n%s", n, out)
	}
}

// A long target is cut, and says it was, rather than wrapped through the
// edges; a pane too narrow for the graph draws nothing inside its border.
func TestViewLongTargetsAreCutAndANarrowPaneDrawsNothing(t *testing.T) {
	longTarget := "app/" + strings.Repeat("very_deep/", 12) + "mod.py"
	out := Render(acted(Action{"edit", longTarget, true}), 80, 40)
	if !strings.Contains(out, "…") {
		t.Fatalf("%s", out)
	}
	pane := actions(acted(Action{"edit", "a.py", true}), 25, 12)
	for i, l := range pane[1 : len(pane)-1] {
		if strings.Trim(l, "│ ") != "" {
			t.Fatalf("row %d of a too-narrow pane drew %q", i+1, l)
		}
	}
	if !strings.Contains(pane[0], "actions") {
		t.Fatalf("the border and title are not drawn: %q", pane[0])
	}
}

// The radius pane is three counts and nothing computed from them: no
// percentage, ratio or total (ADR 0002, ADR 0014).
func TestViewRadiusIsThreeCountsNeverARatio(t *testing.T) {
	out := strings.Join(radius(populated(), 40, 6), "\n")
	for _, want := range []string{"resolved", "47", "candidates", "3", "unresolved", "2", "a floor on reach"} {
		if !strings.Contains(out, want) {
			t.Fatalf("%q missing:\n%s", want, out)
		}
	}
	for _, refused := range []string{"%", "/", "total", "52", "ratio"} {
		if strings.Contains(out, refused) {
			t.Fatalf("the radius pane shows %q:\n%s", refused, out)
		}
	}
}

// No control character in served text reaches the screen, in any pane or
// in the approval modal: the only escape sequences drawn are the colours
// the theme paints with (#841), at every colour depth and on either
// background. Served text is made plain before it is painted, so what it
// carried cannot move the cursor, clear the screen or recolour a row.
func TestViewNoEscapeSequenceReachesTheScreen(t *testing.T) {
	s := populated()
	s.Transcript = append(s.Transcript, Row{Text: "checking\x1b[2K\x1b[Aconverged\r\x07 done", Role: RoleNote})
	s.Edits = append(s.Edits, Edit{Path: "a\x1b]0;x\x07.py"})
	s.Checks = append(s.Checks, Check{Name: "tests\x1b[31m", Passed: false})
	s.Actions = []Action{{Tool: "edit\x1b", Target: "\x9b2J", Wrote: true}}
	s.Input = "typed\x1b"
	s.Outcome = "failed: \x1b[1mbold"
	s.Stderr = []string{"probe\x1b]0;t\x07 done"}
	s.Running = true
	s.Now = Now{Phase: Phase{Kind: "tool", Turn: 2, Tool: "edit\x1b[31m", Target: "a\x9b2J.py"},
		Runtime: "ollama\x1b[2K", Root: "/w/re\x07po"}
	for _, depth := range themetest.Depths {
		for _, dark := range []bool{true, false} {
			th := theme.New(themetest.Renderer(t, depth, dark))
			s.Theme = th
			screens := map[string]string{
				"render": Render(s, 100, 40),
				"modal":  Modal(th, "edit a\x1b[2J.py", "+ok\x1b]0;t\x07\n-\x9b2Jgone", 0, 100, 40),
				"asking": Asking(s, "edit a\x1b[2J.py", "+ok\x1b]0;t\x07\n-\x9b2Jgone", 0, 100, 40),
			}
			for name, out := range screens {
				rest := unpainted(th, out)
				for _, r := range rest {
					if r != '\n' && unicode.IsControl(r) {
						t.Fatalf("%s, %s, dark %v: control character %U reached the screen:\n%q", name, depth, dark, r, out)
					}
				}
				if depth == "TrueColor" && !strings.Contains(out, "\x1b[") {
					t.Fatalf("%s, dark %v: nothing was painted at truecolor, so the check above proved nothing:\n%q", name, dark, out)
				}
			}
			if !strings.Contains(unpainted(th, screens["render"]), "checking") {
				t.Fatalf("%s, dark %v: the line's text was lost with its control characters:\n%s", depth, dark, screens["render"])
			}
		}
	}
}
