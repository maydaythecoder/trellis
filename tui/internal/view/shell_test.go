package view

import (
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// update rewrites the golden renders instead of comparing them:
// go test ./internal/view/ -run TestShellGoldenRenders -update
var update = flag.Bool("update", false, "rewrite the shell's golden renders in testdata/")

// shellState is a run in flight with a row of every kind, for the shell's
// tests and its goldens.
func shellState() State {
	s := populated()
	s.Running = true
	s.Model, s.Tier = "ollama:qwen2.5-coder:32b", "2, native tool calls"
	s.Transcript = []Row{
		{Text: "> fix the failing parser test", Role: RoleYou},
		{Text: "· blast_radius parse_expr", Role: RoleTool, Turn: 1},
		{Text: "    resolved 12 · candidates 1", Role: RoleOutput, Turn: 1},
		{Text: "The parser drops the last token when the input ends in a comment."},
		{Text: "· run_checks  ✓ check passed", Role: RolePassed, Turn: 2},
	}
	s.Actions = []Action{{Tool: "blast_radius", Target: "parse_expr"}, {Tool: "edit", Target: "src/parser.py", Wrote: true}, {Tool: "run_checks"}}
	s.Budget = Budget{Iteration: 3, MaxIterations: u(40), PromptBytes: u(24100), ElapsedSecs: u(83)}
	s.Now = Now{Phase: Phase{Kind: "tool", Turn: 3, Tool: "edit", Target: "src/parser.py"},
		Runtime: "ollama · qwen2.5-coder:32b", Root: "/work/trellis-demo"}
	return s
}

// #851: the shell is the same at every size: the Trail on the first row,
// where the person is and how deep, the Now strip under it, and the status
// bar with the live keys on the last row; every row is the screen's width.
// Inside a pane the Trail names it and says how to come back out.
func TestShellTrailFirstStatusLastAt120x40And80x24(t *testing.T) {
	for _, size := range [][2]int{{120, 40}, {80, 24}} {
		w, h := size[0], size[1]
		for _, open := range []Panel{"", PanelActions} {
			s := shellState()
			s.Nav = Nav{Open: open, Selected: open}
			rows := strings.Split(Render(s, w, h), "\n")
			if len(rows) != h {
				t.Fatalf("%dx%d, open %q: %d rows", w, h, open, len(rows))
			}
			for i, r := range rows {
				if Width(r) != w {
					t.Fatalf("%dx%d, open %q: row %d is %d cells:\n%s", w, h, open, i, Width(r), strings.Join(rows, "\n"))
				}
			}
			top, now, last := rows[0], rows[1], rows[h-1]
			wantTrail, wantDepth, wantKeys := " Run ", "L1 / 4", "[TAB] select  [⏎] in  [^C] stop"
			if open != "" {
				wantTrail, wantDepth, wantKeys = " Run ⟩ actions", "L2 / 4  [ESC] out", "[ESC] out  [^C] stop"
			}
			if !strings.HasPrefix(top, wantTrail) || !strings.Contains(top, wantDepth) || !strings.Contains(top, "TRELLIS") {
				t.Fatalf("%dx%d, open %q: the Trail row reads %q", w, h, open, top)
			}
			if !strings.HasPrefix(now, " ● RUNNING") {
				t.Fatalf("%dx%d, open %q: the Now strip is not under the Trail: %q", w, h, open, now)
			}
			if !strings.Contains(last, "● 1 pending") || (w >= FoldWidth && !strings.Contains(last, wantKeys)) || !strings.Contains(last, "[^C] stop") {
				t.Fatalf("%dx%d, open %q: the status bar reads %q", w, h, open, last)
			}
		}
	}
}

// #851: below FoldWidth by FoldHeight the right column folds: the
// transcript takes the whole width, and the panes are one row under it,
// each with how many rows it holds, the selected one in brackets with the
// key that goes in. Wide and tall enough, the column stands beside the
// transcript and there is no folded row.
func TestShellFoldsTheRightColumnAt80(t *testing.T) {
	s := shellState()
	out := Render(s, 80, 24)
	rows := strings.Split(out, "\n")
	if strings.Contains(out, "┌ actions") || strings.Contains(out, "┌ radius") {
		t.Fatalf("a pane stands beside the transcript at 80 columns:\n%s", out)
	}
	if !strings.HasPrefix(rows[2], "┌ transcript") || !strings.HasSuffix(rows[2], "┐") {
		t.Fatalf("the transcript does not take the whole width: %q", rows[2])
	}
	folded := ""
	for _, r := range rows {
		if strings.Contains(r, "actions 3 · edits 1 · checks 1") {
			folded = r
		}
	}
	if folded == "" || !strings.Contains(folded, "radius") || !strings.Contains(folded, "where") || !strings.Contains(folded, "[TAB] select") {
		t.Fatalf("no folded row naming every pane:\n%s", out)
	}
	s.Nav.Selected = PanelEdits
	if out := Render(s, 80, 24); !strings.Contains(out, "[edits 1]") || !strings.Contains(out, "[⏎] in") {
		t.Fatalf("the selected pane is not marked:\n%s", out)
	}
	if out := Render(shellState(), 120, 40); !strings.Contains(out, "┌ actions") || strings.Contains(out, "actions 3 · edits 1") {
		t.Fatalf("at 120x40 the column folded:\n%s", out)
	}
	if out := Render(shellState(), 120, FoldHeight-1); strings.Contains(out, "┌ actions") {
		t.Fatalf("under FoldHeight rows the column did not fold:\n%s", out)
	}
}

// #851: a pane Tab selected is drawn in signal with heavy corners, as the
// faces doc draws a selected panel; inside it, the rail down the left lists
// every pane's title downward, and the pane takes the rest of the stage.
func TestShellSelectedPaneAndTheRail(t *testing.T) {
	s := shellState()
	s.Nav.Selected = PanelChecks
	out := Render(s, 120, 40)
	if !strings.Contains(out, "┏ checks") || strings.Contains(out, "┏ actions") {
		t.Fatalf("the selected pane is not the heavy one:\n%s", out)
	}
	s.Nav.Open = PanelChecks
	rows := strings.Split(Render(s, 120, 40), "\n")
	if !strings.HasPrefix(rows[2], " w ┌ checks") {
		t.Fatalf("the rail and the open pane do not start the stage: %q", rows[2])
	}
	down := ""
	for _, r := range rows[2:12] {
		down += strings.TrimSpace(string([]rune(r)[:3]))
	}
	if !strings.HasPrefix(down, "where") {
		t.Fatalf("the rail does not write the first title downward: %q", down)
	}
}

// #851: the shell's renders at 80x24 and 120x40, at the top level and in a
// pane, match the files in testdata, so a change to the layout is a change
// to a file a reviewer reads. Run with -update to write them again.
func TestShellGoldenRenders(t *testing.T) {
	for _, c := range []struct {
		name string
		w, h int
		open Panel
	}{
		{"run-80x24", 80, 24, ""},
		{"run-120x40", 120, 40, ""},
		{"actions-80x24", 80, 24, PanelActions},
		{"actions-120x40", 120, 40, PanelActions},
		// The code level and a file's diff (#853), from codeState.
		{"edits-80x24", 80, 24, PanelEdits},
		{"edits-120x40", 120, 40, PanelEdits},
		{"diff-80x24", 80, 24, PanelEdits},
		{"diff-120x40", 120, 40, PanelEdits},
		// The summary level (#854), from summaryState.
		{"summary-80x24", 80, 24, PanelSummary},
		{"summary-120x40", 120, 40, PanelSummary},
	} {
		s := shellState()
		s.Nav = Nav{Open: c.open, Selected: c.open}
		switch c.open {
		case PanelEdits:
			s = codeState()
			s.Nav = Nav{Open: c.open, Selected: c.open, Diff: strings.HasPrefix(c.name, "diff")}
		case PanelSummary:
			s = summaryState()
		}
		got := Render(s, c.w, c.h) + "\n"
		path := filepath.Join("testdata", "shell-"+c.name+".golden")
		if *update {
			if err := os.WriteFile(path, []byte(got), 0o644); err != nil {
				t.Fatal(err)
			}
			continue
		}
		want, err := os.ReadFile(path)
		if err != nil {
			t.Fatalf("%v (write it with -update)", err)
		}
		if got != string(want) {
			t.Fatalf("%s differs from %s (rewrite it with -update if the change is meant):\n%s", c.name, path, got)
		}
	}
}
