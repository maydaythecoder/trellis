package view

import (
	"fmt"
	"strings"
	"testing"
	"unicode"

	"github.com/charmbracelet/lipgloss"

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
	if strings.Contains(out, "tok") || strings.Contains(out, "queued") || !strings.Contains(out, "TURNS █░░░░░░░░░░░░░░░ 3/40") {
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

// A figure trellis did not report is Dash, never drawn as 0 (#641, #911);
// the turns are a Meter against the run's own ceiling once it is known,
// the prompt is in kB, and the elapsed time is a clock, this view's own.
func TestBudgetLineNamesWhatWasNotReported(t *testing.T) {
	s := State{Running: true, Budget: Budget{Iteration: 1}}
	if got := BudgetLine(s); got != "TURNS 1/—  ·  PROMPT —  ·  TIME 0:00*" {
		t.Fatalf("got %q", got)
	}
	s.Budget = Budget{Iteration: 2, MaxIterations: u(40), PromptBytes: u(8400), ElapsedSecs: u(18)}
	if got := BudgetLine(s); got != "TURNS █░░░░░░░░░░░░░░░ 2/40  ·  PROMPT 8.4 kB  ·  TIME 0:18*" {
		t.Fatalf("got %q", got)
	}
}

// The six regions are on screen at 80x24, with the budget's turns against
// their own ceiling, the prompt's size, and the elapsed time.
func TestViewSixRegionsAtEightyByTwentyFour(t *testing.T) {
	out := Render(populated(), 80, 24)
	for _, want := range []string{"TRELLIS", "transcript", "edits", "checks", "radius", "budget",
		"Reading blast radius", "PROMPT 8.4 kB", "TURNS █░░░░░░░░░░░░░░░ 2/40", "TIME 0:18*"} {
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
	// The Now strip says the run ended (#850); the budget row keeps the
	// turns it used against its ceiling.
	if !strings.Contains(out, "last run: did not converge; worktree restored: true") ||
		!strings.Contains(out, "TURNS ████████░░░░░░░░ 3/6") {
		t.Fatalf("outcome:\n%s", out)
	}
}

// The task line shows what is typed with a block cursor, the key that
// sends it and the one that clears it, and the status bar the one that
// leaves, each as its keycap (#911).
func TestViewTaskLineShowsWhatIsTypedAndHowToSendIt(t *testing.T) {
	s := populated()
	s.Input = "fix the type error"
	out := Render(s, 80, 24)
	for _, want := range []string{"fix the type error█", "[⏎] run", "[ESC] clear", "[^C] leave"} {
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
			// Folded (#851), the panes are a row under the transcript and
			// the radius is reached by going in.
			if !folded(w, h) && (!strings.Contains(out, "resolved") || !strings.Contains(out, "a floor on reach")) {
				t.Fatalf("%dx%d, %d offered: the radius pane was cut:\n%s", w, h, n, out)
			}
			more := fmt.Sprintf("… %d more", n-(room-1))
			if n > room && !strings.Contains(out, more) {
				t.Fatalf("%dx%d, %d offered: %q missing:\n%s", w, h, n, more, out)
			}
			if n <= room && strings.Contains(out, " more") {
				t.Fatalf("%dx%d, %d offered fit, yet a row says more:\n%s", w, h, n, out)
			}
			if n > 0 && !strings.Contains(out, "[TAB] src/file_00.py") {
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
// a run that only looked says so, ✎ 0 (#911); one action is not
// "1 actions".
func TestViewActionsCountWritesNotRefusals(t *testing.T) {
	out := Render(acted(Action{"edit", "app/calc.py", false}, Action{"edit", "app/calc.py", false}, Action{"edit", "app/calc.py", true}), 100, 40)
	if !strings.Contains(out, "3 actions") || !strings.Contains(out, "  ✎ 1 ") || !strings.Contains(out, "✎ edit app/calc.py") {
		t.Fatalf("%s", out)
	}
	out = Render(acted(Action{"read_file", "app/calc.py", false}), 100, 40)
	if !strings.Contains(out, "1 action") || strings.Contains(out, "1 actions") || !strings.Contains(out, "  ✎ 0 ") {
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

// codeState is a run in flight that has written src/parser.py twice, each
// edit with its counts and diff, and created tests/test_parser.py with
// write_file, which sends neither; and that asks to edit src/lexer.py, a
// question waiting behind the one on screen.
func codeState() State {
	s := shellState()
	first := "--- src/parser.py\n+++ src/parser.py\n@@ -11,3 +11,3 @@\n def parse_expr(tokens):\n-    return tokens[:-1]\n+    return tokens\n \n"
	second := "--- src/parser.py\n+++ src/parser.py\n@@ -20,1 +20,3 @@\n # the end\n+def parse_comment(tokens):\n+    return []\n"
	s.Edits = []Edit{
		{Path: "src/parser.py", Turn: 3, Added: u(1), Removed: u(1), Diff: &first},
		{Path: "tests/test_parser.py", Turn: 4},
		{Path: "src/parser.py", Turn: 5, Added: u(2), Removed: u(0), Diff: &second},
	}
	s.Asked = []Asked{{ID: "a1", Question: "edit src/lexer.py (+1 -1)",
		Detail: "--- src/lexer.py\n+++ src/lexer.py\n  def lex(src):\n-     return src.split()\n+     return src.split(' ')"}}
	return s
}

// rowWith is the first row of out that holds every one of parts, or "".
func rowWith(out string, parts ...string) string {
	for _, r := range strings.Split(out, "\n") {
		all := true
		for _, p := range parts {
			all = all && strings.Contains(r, p)
		}
		if all {
			return r
		}
	}
	return ""
}

// #853: the code level, one burrow in from the run, is one row per file in
// the order it was first touched, with its two line counts summed over its
// writes and never one figure, and the state word the desktop's Code level
// gives it (#831). The risks: a file written twice listed twice, or with
// only its last edit's counts; a file write_file created, which trellis
// sends no counts for, drawn as +0 −0 rather than as unreported; a file a
// question waits on left out until the run writes it; and an ended run's
// files called anything trellis did not say. Folded at 80 columns the
// level is the same rows.
func TestCodeLevelListsFilesWithLineCounts(t *testing.T) {
	for _, size := range [][2]int{{120, 40}, {80, 24}} {
		s := codeState()
		s.Nav = Nav{Open: PanelEdits, Selected: PanelEdits}
		out := Render(s, size[0], size[1])
		for _, want := range [][]string{
			{"› src/parser.py", "+3 −1", "● in the worktree"},
			{"tests/test_parser.py", Dash, "● in the worktree"},
			{"src/lexer.py", "+1 −1", "● awaiting your approval"},
		} {
			if rowWith(out, want...) == "" {
				t.Fatalf("%v: no row holds %q:\n%s", size, want, out)
			}
		}
		n := 0
		for _, r := range strings.Split(out, "\n") {
			if strings.Contains(r, "src/parser.py") && strings.Contains(r, "│") {
				n++
			}
		}
		if n != 1 {
			t.Fatalf("%v: src/parser.py is listed %d times, want once:\n%s", size, n, out)
		}
		if strings.Contains(out, "+0 −0") || strings.Contains(out, "+0") {
			t.Fatalf("%v: a count trellis did not send is drawn as 0:\n%s", size, out)
		}
		// At 80 columns the status bar keeps the worktree over the keys
		// (statusBar), so the keys are asked of the wider screen only.
		if !strings.HasPrefix(out, " Run ⟩ edits") || !strings.Contains(out, "L2 / 4") || (size[0] > 80 && !strings.Contains(out, "[⏎] diff")) {
			t.Fatalf("%v: the Trail or the keys do not say where the code level is:\n%s", size, out)
		}
		if strings.Index(out, "src/parser.py") > strings.Index(out, "tests/test_parser.py") ||
			strings.Index(out, "tests/test_parser.py") > strings.Index(out, "src/lexer.py") {
			t.Fatalf("%v: the files are not in the order first touched:\n%s", size, out)
		}
	}

	// Ended: what run_end said became of the writes, and nothing else.
	for _, c := range []struct {
		outcome, line string
		restored      *bool
		word          string
	}{
		{"converged", "converged", nil, "left in the tree"},
		{"interrupted", "interrupted; worktree restored", ptr(true), "rolled back"},
		{"not_converged", "not converged; worktree left", ptr(false), "left in the tree"},
		{"error", "error; restored not reported", nil, "not reported"},
		{"", "", nil, "not reported"},
	} {
		s := codeState()
		s.Running, s.Asked = false, nil
		s.Now = Now{Outcome: c.outcome}
		s.Outcome, s.Restored = c.line, c.restored
		s.Nav = Nav{Open: PanelEdits, Selected: PanelEdits}
		out := Render(s, 120, 40)
		if rowWith(out, "src/parser.py", "● "+c.word) == "" || strings.Contains(out, "src/lexer.py") {
			t.Fatalf("%q restored %v: want %q and no file for a question its run no longer waits on:\n%s", c.outcome, c.restored, c.word, out)
		}
		if c.word == "not reported" {
			s.Nav.Diff = true
			if d := Render(s, 120, 40); !strings.Contains(d, "did not say") && !strings.Contains(d, "before a run_end") {
				t.Fatalf("%q: the diff level does not say why the state is not reported:\n%s", c.outcome, d)
			}
		}
	}
}

func ptr(b bool) *bool { return &b }

// #853: a file's diff, one level below the code level, has + and - in the
// sign column and the colour on the text only (the Diff card): added text
// in diff-add, removed in diff-del, each @@ header in diff-hunk, context
// as text, and a question's waiting hunk coloured the same way. The risks:
// the colour on the sign alone or on the whole row, a line coloured by
// what its words say, and the diff, which is repository text and model
// output (rule 10), moving the cursor or retitling the terminal through
// an escape sequence in a line.
func TestDiffColoursLinesByRole(t *testing.T) {
	for _, depth := range themetest.Depths {
		for _, dark := range []bool{true, false} {
			th := theme.New(themetest.Renderer(t, depth, dark))
			s := codeState()
			s.Theme = th
			planted := "--- src/parser.py\n+++ src/parser.py\n@@ -1,1 +1,1 @@\n-old\x1b]0;pwned\x07 line\n+new\x1b[2J line \u009b2J\n"
			s.Edits = append(s.Edits, Edit{Path: "src/parser.py", Turn: 6, Added: u(1), Removed: u(1), Diff: &planted})
			s.Nav = Nav{Open: PanelEdits, Selected: PanelEdits, File: "src/parser.py", Diff: true}
			out := Render(s, 120, 40)
			for _, r := range unpainted(th, out) {
				if r != '\n' && unicode.IsControl(r) {
					t.Fatalf("%s, dark %v: control character %U from the diff reached the screen:\n%q", depth, dark, r, out)
				}
			}
			if !strings.Contains(unpainted(th, out), "new[2J line 2J") {
				t.Fatalf("%s, dark %v: the line's text was lost with its control characters:\n%s", depth, dark, unpainted(th, out))
			}
			if depth != "TrueColor" {
				continue
			}
			for _, c := range []struct {
				sign, text string
				style      lipgloss.Style
			}{
				{"+", "    return tokens", th.DiffAdd},
				{"-", "    return tokens[:-1]", th.DiffDel},
				{"+", "def parse_comment(tokens):", th.DiffAdd},
				{" ", "def parse_expr(tokens):", th.Ink},
			} {
				want := th.Ink.Render(" "+c.sign+" ") + c.style.Render(c.text)
				if c.sign == " " {
					// A context line is text from its sign to its end.
					want = th.Ink.Render("   " + c.text)
				}
				if !strings.Contains(out, want) {
					t.Fatalf("dark %v: %q %q is not drawn with the sign as text and the text in its role:\n%q", dark, c.sign, c.text, out)
				}
			}
			for _, h := range []string{"@@ -11,3 +11,3 @@ · turn 3", "@@ -20,1 +20,3 @@ · turn 5"} {
				if !strings.Contains(out, th.DiffHunk.Render(" "+h)) {
					t.Fatalf("dark %v: %q is not in diff-hunk:\n%q", dark, h, out)
				}
			}
			if strings.Contains(out, "test_parser") || strings.Contains(out, "lexer") {
				t.Fatalf("dark %v: another file's write is in this file's diff:\n%q", dark, out)
			}

			// The question on src/lexer.py, on screen and set aside at its
			// diff: its lines coloured the same way, and its keys in the
			// pane.
			s.Nav.File, s.Reviewing, s.Asked[0].OnScreen = "src/lexer.py", true, true
			out = Render(s, 120, 40)
			for _, want := range []string{
				th.Ink.Render(" + ") + th.DiffAdd.Render("    return src.split(' ')"),
				th.Ink.Render(" - ") + th.DiffDel.Render("    return src.split()"),
			} {
				if !strings.Contains(out, want) {
					t.Fatalf("dark %v: the waiting hunk's line %q is not coloured by its role:\n%q", dark, want, out)
				}
			}
			if rest := unpainted(th, out); !strings.Contains(rest, "[y] accept this change  [n] reject") || !strings.Contains(rest, "awaiting your approval") {
				t.Fatalf("dark %v: the question set aside here does not say how to answer it:\n%s", dark, rest)
			}
		}
	}
}

// summaryState is codeState's run once it has converged, at the summary
// level: checks of two kinds, one run with no duration_ms; the budget with
// every ceiling run budget sends; two steers, one delivered and one not;
// the trail run_end named; and the model's closing words.
func summaryState() State {
	s := codeState()
	s.Running, s.Asked = false, nil
	exit := int64(0)
	s.Now = Now{Outcome: "converged", Exit: &exit, EngineClock: true}
	s.Outcome = "converged"
	s.Checks = []Check{
		{Name: "Typecheck", Passed: true, Turn: 2, TookMs: u(120)},
		{Name: "Tests", Passed: false, Turn: 3, TookMs: u(340)},
		{Name: "Typecheck", Passed: true, Turn: 6},
	}
	s.Budget = Budget{Iteration: 6, MaxIterations: u(40), PromptBytes: u(24100), ElapsedSecs: u(83),
		MaxWallClockMs: u(321000), MaxTokens: u(54321), ContextBytes: u(123456), TokensUsed: u(540)}
	words := "Fixed the parser: the last token is kept.\nThe comment case has a test."
	s.Summary = Summary{
		RunID: "r-20261026-1",
		Steers: []Queued{
			{Kind: "steer", Text: "use the helper", Fate: FateAfter, After: 2},
			{Kind: "steer", Text: "keep the old name", Fate: FateNot, Why: "the run ended first"},
		},
		Closing: Closing{Words: &words},
	}
	s.Nav = Nav{Open: PanelSummary, Selected: PanelSummary}
	return s
}

// summaryText is the whole summary level at width w, every row of it: the
// level drawn tall enough that nothing scrolls out.
func summaryText(s State, w int) string {
	return strings.Join(summary(s, w, 200), "\n")
}

// #854: the summary names how the run ended in run_end's own outcome word
// and exit code, and lists the checks by kind, each run of a kind under it
// with its turn, the verdict trellis sent and its turn's time. The risks:
// an outcome drawn while the run is in flight, which has none; a kind
// listed once per run rather than once; a check whose turn sent no
// duration_ms drawn as 0 ms; and a level that cannot be reached at 80
// columns, where the right column folds.
func TestSummaryShowsOutcomeExitAndChecksByKind(t *testing.T) {
	s := summaryState()
	for _, w := range []int{80 - railW, 120 - railW} {
		out := summaryText(s, w)
		for _, want := range [][]string{
			{"how the run ended"},
			{"outcome", "● converged"},
			{"exit", "0"},
			{"turn 2  passed  120 ms"},
			{"turn 6  passed  " + Dash, "no duration_ms"},
			{"turn 3  failed  340 ms"},
		} {
			if rowWith(out, want...) == "" {
				t.Fatalf("%d: no row holds %q:\n%s", w, want, out)
			}
		}
		if strings.Count(out, "│   Typecheck ") != 1 || strings.Count(out, "│   Tests ") != 1 {
			t.Fatalf("%d: each kind is not listed once:\n%s", w, out)
		}
		if strings.Index(out, "Typecheck") > strings.Index(out, "turn 6") || strings.Index(out, "Tests") > strings.Index(out, "turn 3") {
			t.Fatalf("%d: a run is not under its kind:\n%s", w, out)
		}
		if strings.Contains(out, "0 ms") && !strings.Contains(out, "120 ms") {
			t.Fatalf("%d: a duration trellis did not send is drawn as 0:\n%s", w, out)
		}
	}

	// In flight: so far, and no outcome or exit code but Dash with why.
	s.Running, s.Outcome, s.Now = true, "", Now{}
	out := summaryText(s, 80-railW)
	if rowWith(out, "so far") == "" || rowWith(out, "outcome", Dash, "outcome and exit code come") == "" || rowWith(out, "exit", Dash) == "" ||
		strings.Contains(out, "converged") || strings.Contains(out, "how the run ended") {
		t.Fatalf("the run in flight has an outcome:\n%s", out)
	}

	// Reached at 80x24, folded: the Trail names the level, and its first
	// rows are on screen.
	for _, size := range [][2]int{{80, 24}, {120, 40}} {
		out := Render(summaryState(), size[0], size[1])
		if !strings.HasPrefix(out, " Run ⟩ summary") || !strings.Contains(out, "L2 / 4") || rowWith(out, "outcome", "● converged") == "" {
			t.Fatalf("%v: the summary level is not where the Trail says:\n%s", size, out)
		}
	}
}

// #854, CLAUDE.md rule 4: Verified and Reviewed are two rows, each its own
// figure, never one number, a total or a share of the other. Both are
// counted only in the trail, which trellis rpc does not send and this view
// does not read, so each is Dash, never 0, with the reason beside it and
// the trail's id for `trellis log`. The risk is a summary that adds them,
// shows one as a fraction of the other, or prints 0 for what was never
// reported.
func TestSummaryKeepsVerifiedAndReviewedApart(t *testing.T) {
	for _, c := range []struct {
		name string
		edit func(*State)
		why  string
	}{
		{"ended", func(*State) {}, "`trellis log` lists run r-20261026-1"},
		{"no trail", func(s *State) { s.Summary.RunID = "" }, "run_end named no trail"},
		{"so far", func(s *State) { s.Running, s.Outcome = true, "" }, "which trellis writes at run_end"},
	} {
		s := summaryState()
		c.edit(&s)
		out := summaryText(s, 80-railW)
		verified, reviewed := rowWith(out, "Verified", Dash), rowWith(out, "Reviewed", Dash)
		if verified == "" || reviewed == "" || verified == reviewed {
			t.Fatalf("%s: Verified and Reviewed are not two rows, each with its Dash:\n%s", c.name, out)
		}
		for _, r := range strings.Split(out, "\n") {
			if strings.Contains(r, "Verified") && strings.Contains(r, "Reviewed") {
				t.Fatalf("%s: one row holds both: %q", c.name, r)
			}
		}
		at := strings.Index(out, "Verified")
		section := out[at:strings.Index(out, " RADIUS")]
		for _, refused := range []string{"%", "/", "total", "ratio", " 0 "} {
			if strings.Contains(section, refused) {
				t.Fatalf("%s: the modes show %q:\n%s", c.name, refused, section)
			}
		}
		// The reasons wrap, so they are read with the box's sides and the
		// spaces between rows taken out.
		if strings.Count(strings.Join(strings.Fields(strings.ReplaceAll(section, "│", "")), " "), c.why) != 2 {
			t.Fatalf("%s: each row does not say why it is unreported (%q):\n%s", c.name, c.why, section)
		}
	}
}

// #854: the radius is its three counts, never a ratio (rule 3), and the
// budget is each figure used against the ceiling run budget sent, the
// completion tokens never summed with the prompt (UI rule 5); a ceiling
// trellis did not send is Dash with why, never 0. The steers say what
// became of each in the queue's words.
func TestSummaryRadiusBudgetAndSteers(t *testing.T) {
	out := summaryText(summaryState(), 80-railW)
	for _, want := range [][]string{
		{"resolved", "47"}, {"candidates", "3"}, {"unresolved", "2"}, {"a floor on reach, not a boundary"},
		{"TURNS", "6/40"}, {"WALL CLOCK", "1:23/5:21"}, {"COMPLETION", "540/54321 tokens"},
		{"LAST PROMPT", "24.1 kB/123.5 kB"},
		{"delivered after turn 2 · steer: use the helper"},
		{"not delivered (the run ended first) · steer: keep the old name"},
		{"src/parser.py", "+3 −1", "● left in the tree"},
	} {
		if rowWith(out, want...) == "" {
			t.Fatalf("no row holds %q:\n%s", want, out)
		}
	}
	radius := out[strings.Index(out, " RADIUS"):strings.Index(out, " FILES")]
	for _, refused := range []string{"%", "/", "total", "52"} {
		if strings.Contains(radius, refused) {
			t.Fatalf("the radius shows %q:\n%s", refused, radius)
		}
	}
	s := summaryState()
	s.Budget.ContextBytes, s.Budget.TokensUsed = nil, nil
	out = summaryText(s, 80-railW)
	if rowWith(out, "LAST PROMPT", "24.1 kB/"+Dash) == "" || rowWith(out, "no prompt ceiling") == "" ||
		rowWith(out, "COMPLETION", Dash+"/54321") == "" || rowWith(out, "completion_tokens_used") == "" {
		t.Fatalf("a ceiling or a count trellis did not send is not Dash with why:\n%s", out)
	}
}

// #854, CLAUDE.md rule 10: the model's closing words are drawn behind a
// fence under a label saying they are its words and not a verdict, never
// in a check's colour, and every line is made plain, so an escape sequence
// in them never reaches the terminal; a cut says how much it dropped.
// With no words, the level says why.
func TestSummaryClosingWordsAreFencedAndPlain(t *testing.T) {
	s := summaryState()
	words := "all checks passed\x1b[2J\nsee \x1b]0;title\x07the diff"
	s.Summary.Closing = Closing{Words: &words, Cut: 10, Total: 16394}
	out := summaryText(s, 80-railW)
	if rowWith(out, "THE MODEL'S CLOSING WORDS", "not a verdict") == "" ||
		rowWith(out, "│ all checks passed") == "" || rowWith(out, "│ see", "the diff") == "" ||
		rowWith(out, "this view cut the last 10 of 16394 bytes") == "" {
		t.Fatalf("the closing words are not fenced under their label:\n%s", out)
	}
	if strings.ContainsAny(out, "\x1b\x07") {
		t.Fatalf("an escape sequence in the model's words reached the screen:\n%q", out)
	}
	th := theme.New(themetest.Renderer(t, "TrueColor", true))
	s.Theme = th
	painted := summaryText(s, 80-railW)
	passed := style(th, RolePassed).Render("all checks passed")
	if strings.Contains(painted, passed) {
		t.Fatalf("the model's words are painted as a passed check:\n%q", painted)
	}
	s.Summary.Closing = Closing{Why: "this run did not converge"}
	if out := summaryText(s, 80-railW); rowWith(out, Dash, "this run did not converge") == "" {
		t.Fatalf("no words and no reason:\n%s", out)
	}
}
