package model

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

// worked feeds three writes to two files, and a check, a radius and a
// read: the four panes each get something.
func worked(m *Model) {
	feed(m, turn(1, "blast_radius f", map[string]any{"resolved": 4, "candidates": 1, "unresolved": 0}))
	feed(m, turn(2, "edit a.py", map[string]any{"observation": "applied: true"}))
	feed(m, turn(3, "edit a.py", map[string]any{"observation": "applied: true"}))
	feed(m, turn(4, "write_file b.py", map[string]any{"observation": "created b.py"}))
	feed(m, turn(5, "run_checks", map[string]any{"verdict": "Some(false)", "check_kind": "pytest"}))
}

// #636: after a rolled-back run the bar said "3 edit(s) pending" for files
// the rollback had put back. It follows how the run ended: writes counted
// while the run is in flight, "restored" after a rollback, and, after a
// run that leaves its changes (converged, or one whose ending did not say
// it restored), the files left for review, counted as files.
func TestStatusWorktreeFollowsHowTheRunEnded(t *testing.T) {
	m := running(t)
	worked(m)
	if bar := barOf(m); !strings.Contains(bar, "worktree 3 edit(s) pending") {
		t.Fatalf("%q", bar)
	}
	feed(m, runEnd(map[string]any{"outcome": "not_converged", "exit": 2, "restored": true}))
	if bar := barOf(m); !strings.Contains(bar, "worktree restored") || strings.Contains(bar, "pending") {
		t.Fatalf("after a rollback: %q", bar)
	}

	submit(m, "change it again")
	worked(m)
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if bar := barOf(m); !strings.Contains(bar, "worktree 2 file(s) left for review") {
		t.Fatalf("after a converged run: %q", bar)
	}

	// An ending that did not say it restored is not claimed as restored;
	// one that could not restore is not either.
	for _, end := range []map[string]any{
		{"outcome": "error", "exit": 1, "error": "boom"},
		{"outcome": "not_converged", "exit": 2, "restored": false},
	} {
		m := running(t)
		if feed(m, runEnd(end)); !strings.Contains(barOf(m), "worktree clean") {
			t.Fatalf("%v with no edits: %q", end, barOf(m))
		}
		m = running(t)
		worked(m)
		if feed(m, runEnd(end)); !strings.Contains(barOf(m), "worktree 2 file(s) left for review") {
			t.Fatalf("%v: %q", end, barOf(m))
		}
	}
}

// #636: nothing cleared the panes, so a second run listed the first run's
// edits and checks beside its own. A new task starts with the four panes
// empty and the last run's ending forgotten: a run starts from a clean
// tree.
func TestStatusWorktreeASecondTaskClearsTheFourPanes(t *testing.T) {
	m := running(t)
	worked(m)
	feed(m, runEnd(map[string]any{"outcome": "not_converged", "exit": 2, "restored": true}))
	submit(m, "second")
	if len(m.Actions) != 0 || len(m.Edits) != 0 || len(m.Checks) != 0 || m.Radius != nil {
		t.Fatalf("actions %v edits %v checks %v radius %v", m.Actions, m.Edits, m.Checks, m.Radius)
	}
	if m.restored != nil {
		t.Fatal("the last run's ending is kept for this one")
	}
	if bar := barOf(m); !strings.Contains(bar, "worktree clean") {
		t.Fatalf("%q", bar)
	}
}

// #636: drawn last, the worktree was cut to "work" by a long spec at 80
// columns. It comes before the model now, whole, and the model is the span
// that gives way, saying it was cut.
func TestStatusWorktreeIsWholeAtEightyColumnsWithALongSpec(t *testing.T) {
	spec := "llamacpp:http://127.0.0.1:8080#qwen3-8b"
	spec += strings.Repeat("x", 40-len(spec))
	m, stateID, _ := startup(t)
	respond(m, stateID, "get_state", map[string]any{"model": spec, "tier": "unknown, probing…"})
	bar80 := func() string {
		m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
		return strings.SplitN(m.View(), "\n", 2)[0]
	}
	bar := bar80()
	if !strings.Contains(bar, "worktree clean") || !strings.Contains(bar, "tier unknown, probing…") {
		t.Fatalf("%q", bar)
	}
	if !strings.Contains(bar, "…") || strings.Contains(bar, spec) || len([]rune(bar)) > 80 {
		t.Fatalf("the model is not the span cut: %q", bar)
	}
	// The longest state the bar can say, after a converged run.
	submit(m, "t")
	for _, f := range []string{"a.py", "b.py", "c.py"} {
		feed(m, turn(1, "edit "+f, map[string]any{"observation": "applied: true"}))
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if bar := bar80(); !strings.Contains(bar, "worktree 3 file(s) left for review") {
		t.Fatalf("%q", bar)
	}
}

// /help lists the page keys (#660) and says ^c stops and rolls back a run
// in flight before leaving (#634); the stop is drawn while the run winds
// down, and the program stays until the child has exited.
func TestSlashHelpListsThePageKeysAndTheStop(t *testing.T) {
	m := fresh(t)
	typeLine(m, "/help")
	enter(m)
	help := lines(m)
	for _, want := range []string{
		"PgUp PgDn        ten lines at a time; Home to the start, End back to following",
		"^c               leave; a run in flight is stopped and rolled back first",
	} {
		if !strings.Contains(help, want) {
			t.Fatalf("%q not in help:\n%s", want, help)
		}
	}
	m = running(t)
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	if v := m.View(); !strings.Contains(v, "stopping:") || m.Done {
		t.Fatalf("the stop is not drawn while the run winds down (done %v):\n%s", m.Done, v)
	}
}

// #655: a row cut inside a word reads as a different word ("stopp",
// "boun"). At 80 columns every token of the transcript pane is a token of
// the help text.
func TestSlashHelpHasNoRowEndingMidWord(t *testing.T) {
	m := fresh(t)
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 60})
	typeLine(m, "/help")
	enter(m)
	words := map[string]bool{}
	for _, w := range strings.Fields(lines(m)) {
		words[w] = true
	}
	// The transcript pane: its rows from under its top border, at column
	// 0, to its bottom border.
	var pane []string
	rows := strings.Split(m.View(), "\n")
	if !strings.HasPrefix(rows[1], "┌ transcript") {
		t.Fatalf("no transcript pane at row 1:\n%s", m.View())
	}
	for _, row := range rows[2:] {
		r := []rune(row)
		if r[0] != '│' {
			break
		}
		end := 1
		for end < len(r) && r[end] != '│' {
			end++
		}
		pane = append(pane, string(r[1:end]))
	}
	all := strings.Join(pane, "\n")
	if !strings.Contains(all, "scroll the transcript") {
		t.Fatalf("help is not in the pane:\n%s", all)
	}
	for _, row := range pane {
		for _, tok := range strings.Fields(row) {
			if !words[tok] {
				t.Fatalf("%q is not a word of the help text, in row %q:\n%s", tok, row, all)
			}
		}
	}
}
