package model

import (
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

func warn(msg string) string {
	return jsonLine(map[string]any{"level": "WARN", "fields": map[string]any{"message": msg}})
}

// A turn whose verdict is Some(true) is a line and a passing check; check
// rows come only from Some(true) or Some(false), named by check_kind ("check"
// when empty); None and anything else make no row (ADR 0004).
func TestPanesCheckRowsComeOnlyFromASomeVerdict(t *testing.T) {
	m := running(t)
	feed(m, turn(1, "run_checks", map[string]any{"verdict": "Some(true)", "check_kind": "pytest"}))
	feed(m, turn(2, "run_checks", map[string]any{"verdict": "Some(false)", "check_kind": "mypy"}))
	feed(m, turn(3, "run_checks", map[string]any{"verdict": "Some(false)", "check_kind": ""}))
	feed(m, turn(4, "edit a.py", map[string]any{"verdict": "None"}))
	feed(m, turn(5, "read_file a.py", map[string]any{"verdict": "true"}))
	want := []view.Check{{Name: "pytest", Passed: true}, {Name: "mypy", Passed: false}, {Name: "check", Passed: false}}
	if m.Checks.Len() != len(want) {
		t.Fatalf("checks %+v", m.Checks.Slice())
	}
	for i := range want {
		if m.Checks.Slice()[i] != want[i] {
			t.Fatalf("check %d: %+v, want %+v", i, m.Checks.Slice()[i], want[i])
		}
	}
	m.Update(windowSize(120, 40))
	v := m.View()
	if !strings.Contains(v, "pytest passed") || !strings.Contains(v, "mypy failed") || strings.Contains(v, "mypy passed") {
		t.Fatalf("%s", v)
	}
}

// An edit whose observation has applied: true, and a write_file that
// created its file, are writes and pending edits; a refused edit is an
// action and not an edit, so the panes never contradict each other.
func TestPanesOnlyAnAppliedWriteIsAPendingEdit(t *testing.T) {
	m := running(t)
	feed(m, turn(1, "edit app/calc.py", map[string]any{"observation": "error: old text not found"}))
	feed(m, turn(2, "edit app/calc.py", map[string]any{"observation": "applied: false"}))
	feed(m, turn(3, "edit app/calc.py", map[string]any{"observation": "applied: true\n1 line changed"}))
	feed(m, turn(4, "write_file app/new.py", map[string]any{"observation": "created app/new.py"}))
	feed(m, turn(5, "write_file app/x.py", map[string]any{"observation": "error: exists"}))
	if m.Actions.Len() != 5 {
		t.Fatalf("%d actions, want every turn", m.Actions.Len())
	}
	wrote := 0
	for _, a := range m.Actions.Slice() {
		if a.Wrote {
			wrote++
		}
	}
	if wrote != 2 || m.Edits.Len() != 2 || m.Edits.Slice()[0].Path != "app/calc.py" || m.Edits.Slice()[1].Path != "app/new.py" {
		t.Fatalf("wrote %d, edits %+v", wrote, m.Edits.Slice())
	}
	m.Update(windowSize(120, 40))
	if v := m.View(); !strings.Contains(v, "● 2 pending") || !strings.Contains(v, "~ app/calc.py") {
		t.Fatalf("%s", v)
	}
}

// #635: an approval answer is about the next call, not the edits already
// shown. The loop reports an edit once it has written it and asks before
// the next call, so a.py is in the tree whatever the answer: marking it
// rejected on n told an approve-mode user a written file was not there to
// review. The row keeps the mark it was reported with, on y and on n, and
// the decline is said in the transcript.
func TestPanesAnApprovalAnswerKeepsTheEditsAsReported(t *testing.T) {
	for _, c := range []struct {
		key  rune
		yes  bool
		said string
	}{{'y', true, "approved"}, {'n', false, "declined"}} {
		m := running(t)
		feed(m, turn(1, "edit a.py", map[string]any{"observation": "applied: true"}))
		feed(m, approval("a1", "edit a.py (+1 -1)", "apply the edit?"))
		key(m, c.key)
		if m.Edits.Len() != 1 || m.Edits.Slice()[0] != (view.Edit{Path: "a.py", Status: view.Proposed}) {
			t.Fatalf("%c: %+v", c.key, m.Edits.Slice())
		}
		out := m.TakeOutbox()
		if len(out) != 1 || out[0].Cmd["yes"] != c.yes {
			t.Fatalf("%c sent %v", c.key, kinds(out))
		}
		m.Update(windowSize(100, 30))
		v := m.View()
		if !strings.Contains(v, "~ a.py") || strings.Contains(v, "- a.py") || strings.Contains(v, "+ a.py") {
			t.Fatalf("%c: the edits row was relabelled:\n%s", c.key, v)
		}
		if !strings.Contains(v, "· edit a.py (+1 -1) "+c.said) {
			t.Fatalf("%c: the answer is not in the transcript:\n%s", c.key, v)
		}
	}
}

// A blast_radius turn carries its three counts to the radius pane, kept as
// three numbers and never combined.
func TestPanesRadiusTurnCarriesThreeCounts(t *testing.T) {
	m := running(t)
	feed(m, turn(1, "blast_radius f", map[string]any{"resolved": 47, "candidates": 3, "unresolved": 2}))
	if m.Radius == nil || *m.Radius != (view.Radius{Resolved: 47, Candidates: 3, Unresolved: 2}) {
		t.Fatalf("%+v", m.Radius)
	}
	feed(m, turn(2, "blast_radius g", map[string]any{"resolved": 1}))
	if m.Radius.Resolved != 47 {
		t.Fatal("a partial radius replaced a whole one")
	}
}

// The run's ceiling comes from its run budget event; each turn brings its
// iteration, prompt bytes, tokens, model time and queue wait, and the
// seconds since the run started; ticks of nothing change nothing.
func TestPanesBudgetCarriesTheCeilingTheTurnsFiguresAndElapsed(t *testing.T) {
	m := fresh(t)
	clock := time.Date(2026, 10, 12, 12, 0, 0, 0, time.UTC)
	m.now = func() time.Time { return clock }
	submit(m, "t")
	feed(m, jsonLine(map[string]any{"type": "run_start", "id": "tui-3"}))
	feed(m, jsonLine(map[string]any{"level": "INFO", "fields": map[string]any{"message": "run budget", "max_iterations": 40}}))
	clock = clock.Add(18 * time.Second)
	feed(m, turn(2, "read_file a.py", map[string]any{"prompt_bytes": 8400, "completion_tokens": 30, "model_ms": 25000, "queue_wait_ms": 10000}))
	b := m.Budget
	if b.Iteration != 2 || b.MaxIterations == nil || *b.MaxIterations != 40 || *b.PromptBytes != 8400 ||
		*b.ElapsedSecs != 18 || *b.CompletionTokens != 30 || *b.ModelMs != 25000 || *b.QueueWaitMs != 10000 {
		t.Fatalf("%+v", b)
	}
	m.Update(windowSize(140, 30))
	v := m.View()
	if !strings.Contains(v, "TURNS █░░░░░░░░░░░░░░░ 2/40  ·  PROMPT 8.4 kB  ·  TIME 0:18*  ·  30 tok, 2.0 tok/s  ·  queued 10.0 s") {
		t.Fatalf("%s", v)
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if m.Budget.Iteration != 2 || m.Actions.Len() != 1 {
		t.Fatal("the run's end cleared what it did")
	}
}

// A warning is a transcript line and not a turn: no action, no budget
// step; an INFO event that is not a turn reaches nothing.
func TestPanesAWarningIsALineAndNotATurn(t *testing.T) {
	m := running(t)
	feed(m, warn("no progress; stopping"))
	feed(m, jsonLine(map[string]any{"level": "INFO", "fields": map[string]any{"message": "resolving imports", "call": "Edit"}}))
	if last(m) != "no progress; stopping" || m.Actions.Len() != 0 || m.Budget.Iteration != 0 {
		t.Fatalf("last %q actions %d budget %+v", last(m), m.Actions.Len(), m.Budget)
	}
}

// A turn's observation is shown under its line, at most six lines, each
// indented and plain.
func TestPanesObservationLinesFollowTheTurn(t *testing.T) {
	m := running(t)
	feed(m, turn(1, "read_file a.py", map[string]any{"observation": "1\n2\n3\n4\n5\n6\n7\x1b[2J"}))
	tail := texts(m)[len(m.Transcript)-7:]
	if tail[0] != "· read_file a.py" || tail[1] != "    1" || tail[6] != "    6" {
		t.Fatalf("%q", tail)
	}
}

// Up scrolls back, Down returns and saturates at the end, and a new run is
// followed from its start.
func TestPanesScrollingBackStopsAtTheEndAndResumesFollowing(t *testing.T) {
	m := numbered(t, 200)
	m.Update(keyType(keyDown))
	if m.ScrollBack != 0 {
		t.Fatal("down at the end did not saturate")
	}
	m.Update(keyType(keyUp))
	m.Update(keyType(keyUp))
	if m.ScrollBack != 2 {
		t.Fatalf("scroll back %d", m.ScrollBack)
	}
	submit(m, "t")
	if m.ScrollBack != 0 {
		t.Fatal("a new run is not followed")
	}
}

// #843: Home stopped at the total row count instead of the count less the
// pane's height, so PgDn after Home first spent itself on rows above the
// first and the view did not move: the first press at 80x24, the first
// three at 120x40.
func TestPanesPageDownMovesAtOnceAfterHome(t *testing.T) {
	for _, size := range [][2]int{{80, 24}, {120, 40}} {
		m := numbered(t, 200)
		m.Update(windowSize(size[0], size[1]))
		m.Update(keyType(tea.KeyHome))
		top := m.View()
		if !strings.Contains(top, "line 000") {
			t.Fatalf("%dx%d: home does not show the first line:\n%s", size[0], size[1], top)
		}
		m.Update(keyType(tea.KeyPgDown))
		if moved := m.View(); moved == top || strings.Contains(moved, "line 000") {
			t.Fatalf("%dx%d: the first PgDn after Home did not move the view:\n%s", size[0], size[1], moved)
		}
	}
}

// #843: Up had no top. Each press past it raised the offset, and as many
// Downs went by before the view moved again. Up now stops where Home does,
// and the first Down from there moves.
func TestPanesUpStopsAtTheTop(t *testing.T) {
	m := numbered(t, 200)
	m.Update(keyType(tea.KeyHome))
	top := m.ScrollBack
	for i := 0; i < 5; i++ {
		m.Update(keyType(keyUp))
	}
	if m.ScrollBack != top {
		t.Fatalf("scroll back %d after Up at the top %d", m.ScrollBack, top)
	}
	at := m.View()
	m.Update(keyType(keyDown))
	if m.ScrollBack != top-1 || m.View() == at {
		t.Fatalf("the first Down from the top did not move: scroll back %d, top %d", m.ScrollBack, top)
	}
	// A short transcript has no rows above its first page, so Up holds.
	short := numbered(t, 3)
	short.Update(keyType(keyUp))
	if short.ScrollBack != 0 {
		t.Fatalf("scroll back %d over a transcript that fits", short.ScrollBack)
	}
}

// #843: a window made taller lowers the top; an offset set at the shorter
// size is brought down to it, so the next key moves the view at once.
func TestPanesAResizeKeepsTheOffsetWithinTheTop(t *testing.T) {
	m := numbered(t, 200)
	m.Update(keyType(tea.KeyHome))
	m.Update(windowSize(80, 60))
	if top := view.TranscriptTop(m.viewState(), 80, 60); m.ScrollBack != top {
		t.Fatalf("scroll back %d after growing, top %d", m.ScrollBack, top)
	}
}

// The next run starts with empty panes; the last run's stay until then.
func TestPanesANewRunStartsClean(t *testing.T) {
	m := running(t)
	feed(m, turn(1, "edit a.py", map[string]any{"observation": "applied: true", "verdict": "Some(true)"}))
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if m.Edits.Len() != 1 || m.Checks.Len() != 1 {
		t.Fatal("the panes were cleared at the run's end")
	}
	submit(m, "next")
	if m.Edits.Len()+m.Checks.Len()+m.Actions.Len() != 0 || m.Radius != nil {
		t.Fatal("the last run's panes were carried into this one")
	}
}
