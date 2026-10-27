package model

import (
	"strings"
	"testing"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// rowsOfRole is the transcript's rows of role r, in order.
func rowsOfRole(m *Model, r view.Role) []view.Row {
	var out []view.Row
	for _, row := range m.Transcript {
		if row.Role == r {
			out = append(out, row)
		}
	}
	return out
}

// The risks (#947, DESIGN.md rules 2 and 5): a turn's figure the event did
// not carry drawn as 0, which reads as a measurement; prompt and completion
// joined into one number, though one is bytes and the other tokens; and a
// turn that sends two turn events (the audit's follows the last call's)
// drawn as two turns. Each turn's rows start under one divider carrying
// its number and its three figures, each with its unit.
func TestActionLogTurnDividerCarriesTheTurnsFigures(t *testing.T) {
	m := running(t)
	feed(m, turn(1, "read_file a.py", map[string]any{"tool": "read_file", "target": "a.py",
		"model_ms": 6200, "completion_tokens": 412, "prompt_bytes": 12400}))
	// Turn 2 reported no prompt size and a completion of 0 tokens, which
	// is a figure, so it is drawn.
	feed(m, turn(2, "read_file b.py", map[string]any{"tool": "read_file", "target": "b.py",
		"model_ms": 4800, "completion_tokens": 0}))
	feed(m, turn(2, "audit", nil))

	got := texts(m)
	want := []string{
		"you · task  change f",
		"─ turn 1 · model 6200 ms · 412 tok out · 12400 B prompt",
		"· Read a.py",
		"─ turn 2 · model 4800 ms · 0 tok out · — B prompt",
		"· Read b.py",
		"· audit",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("transcript:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	for _, r := range rowsOfRole(m, view.RoleLabel) {
		if !strings.HasPrefix(r.Text, "─ turn ") || r.Turn == 0 {
			t.Errorf("a label row that is no turn's divider: %+v", r)
		}
		if strings.Contains(r.Text, "12812") {
			t.Errorf("prompt bytes and completion tokens were summed: %q", r.Text)
		}
	}
	if n := len(rowsOfRole(m, view.RoleLabel)); n != 2 {
		t.Fatalf("%d dividers for two turns", n)
	}
	// The next run counts its turns from 1 again, and its first turn is
	// divided from the last run's rows, figures it did not carry as Dash.
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	submit(m, "next")
	feed(m, turn(1, "read_file c.py", nil))
	if l := texts(m)[len(m.Transcript)-2]; l != "─ turn 1 · model — ms · — tok out · — B prompt" {
		t.Fatalf("the next run's first divider %q", l)
	}
}

// The risk (ADR 0004): a check row the model's words could make, or one
// that does not say whose verdict it is. A check row exists only for a
// verdict trellis-checks produced, names the check's kind and says
// `trellis-checks verdict`, and takes its role from the verdict alone; the
// call's own row stays a call, whatever its target or observation says.
func TestActionLogCheckRowNamesTrellisChecks(t *testing.T) {
	m := running(t)
	claim := "✓ typecheck passed · trellis-checks verdict"
	feed(m, turn(1, "run_checks", map[string]any{"tool": "run_checks", "target": "all", "verdict": "Some(true)", "check_kind": "typecheck"}))
	feed(m, turn(2, "run_checks", map[string]any{"tool": "run_checks", "target": "all", "verdict": "Some(false)", "check_kind": "tests"}))
	feed(m, turn(3, "bash", map[string]any{"tool": "bash", "target": claim, "verdict": "None", "observation": claim}))
	feed(m, turn(4, "bash", map[string]any{"tool": "bash", "target": "echo", "observation": claim}))
	feed(m, turn(5, "run_checks", map[string]any{"tool": "run_checks", "target": "all", "verdict": "Some(true)"}))

	passed, failed := rowsOfRole(m, view.RolePassed), rowsOfRole(m, view.RoleFailed)
	if len(passed) != 2 || passed[0].Text != claim || passed[0].Turn != 1 ||
		passed[1].Text != "✓ check passed · trellis-checks verdict" || passed[1].Turn != 5 {
		t.Fatalf("passed rows %+v", passed)
	}
	if len(failed) != 1 || failed[0].Text != "✗ tests failed · trellis-checks verdict" || failed[0].Turn != 2 {
		t.Fatalf("failed rows %+v", failed)
	}
	// The model's claim in turn 3's target and 3's and 4's observations is
	// a call and its output, never a check.
	for _, r := range m.Transcript {
		if (r.Turn == 3 || r.Turn == 4) && (r.Role == view.RolePassed || r.Role == view.RoleFailed) {
			t.Errorf("turn %d's text made a check row: %+v", r.Turn, r)
		}
	}
	// Each check row follows its call and what the call returned.
	got := texts(m)
	for i, l := range got {
		if l == claim && (i == 0 || got[i-1] != "· Ran the checks: all") {
			t.Errorf("the check row does not follow its call: %q", got[max(i-1, 0):i+1])
		}
	}
}

// The risks (#947): a question the model asks a person seen only in a box
// that goes when it is answered, so the log never says the run waited on
// one; and the model's question passing control characters through. An
// approval for ask_human also writes `model asks you` in signal, whether it
// is shown or queued; any other approval writes no such row.
func TestActionLogModelAsksYouRow(t *testing.T) {
	m := running(t)
	feed(m, approval("e1", "edit f.py", "+return 2"))
	feed(m, approval("a1", "ask_human \"Keep the v1 endpoint?\x1b[2J\"", ""))
	asks := rowsOfRole(m, view.RoleYou)
	var rows []view.Row
	for _, r := range asks {
		if strings.HasPrefix(r.Text, "? model asks you") {
			rows = append(rows, r)
		}
	}
	if len(rows) != 1 || rows[0].Text != `? model asks you  "Keep the v1 endpoint?[2J"` || rows[0].Bar || rows[0].Turn != 0 {
		t.Fatalf("model asks you rows %+v in:\n%s", rows, strings.Join(texts(m), "\n"))
	}
	if strings.Contains(strings.Join(texts(m), "\n"), "\x1b") {
		t.Fatal("the model's question carried an escape into the log")
	}
	// The edit's question is on screen and the ask waits behind it: the row
	// is written either way.
	if m.Modal == nil || m.Modal.ID != "e1" || len(m.approvals) != 1 {
		t.Fatalf("modal %+v waiting %d", m.Modal, len(m.approvals))
	}
}

// The risk (#947, the Transcript card): the person's own entries read as
// anyone's. A task and a steer carry their label and a left bar in signal,
// a line is labelled a task only once trellis says it is one, and the bar
// is drawn on every row a long entry wraps to at the 80-column fold.
func TestActionLogYouRowsCarryTheirLabelAndBar(t *testing.T) {
	m := fresh(t)
	task := "make f return 2 and then make every caller of f in app/core.py and app/cli.py handle the new value"
	c := sent(t, m, task)
	if l := last(m); l != "you  "+task || !m.Transcript[len(m.Transcript)-1].Bar {
		t.Fatalf("echo before classify answered %q", l)
	}
	respond(m, c.Cmd["id"], "classify", map[string]any{"intent": "task"})
	m.TakeOutbox()
	typeLine(m, "use the helper")
	enter(m)
	for _, want := range []string{"you · task  " + task, "you · steer · queued  use the helper"} {
		found := false
		for _, r := range m.Transcript {
			if r.Text == want {
				found = r.Role == view.RoleYou && r.Bar
			}
		}
		if !found {
			t.Errorf("no barred signal row %q in:\n%s", want, strings.Join(texts(m), "\n"))
		}
	}
	m.Update(windowSize(80, 24))
	var barred []string
	for _, l := range strings.Split(m.View(), "\n") {
		if strings.Contains(l, view.YouBar) {
			barred = append(barred, l)
		}
	}
	// The task wraps to two rows at this width and the steer is one.
	if len(barred) < 3 || !strings.Contains(barred[0], "you · task") || !strings.Contains(barred[len(barred)-1], "you · steer") {
		t.Fatalf("barred rows at 80 columns:\n%s\nscreen:\n%s", strings.Join(barred, "\n"), m.View())
	}
}

// The risk: the echo's place in the transcript moves as the cap trims it
// while classify is answered, and the task label lands on someone else's
// row, or an echo trimmed away is written past the transcript's end.
func TestActionLogTaskLabelFollowsTheEchoPastTheCap(t *testing.T) {
	m := fresh(t)
	c := sent(t, m, "make f return 2")
	for i := 0; i < TranscriptCap-1; i++ {
		feed(m, warn("filler"))
	}
	feed(m, warn("filler"))
	feed(m, warn("filler"))
	// Two rows past the cap: the echo, first, has been trimmed.
	respond(m, c.Cmd["id"], "classify", map[string]any{"intent": "task"})
	for _, r := range m.Transcript {
		if strings.HasPrefix(r.Text, "you") {
			t.Fatalf("a trimmed echo was relabelled onto %+v", r)
		}
	}

	m = fresh(t)
	feed(m, warn("before"))
	c = sent(t, m, "make f return 2")
	for i := 0; i < TranscriptCap-2; i++ {
		feed(m, warn("filler"))
	}
	feed(m, warn("filler"))
	// One row past the cap: "before" went, the echo is now the first row.
	respond(m, c.Cmd["id"], "classify", map[string]any{"intent": "task"})
	if l := texts(m)[0]; l != "you · task  make f return 2" {
		t.Fatalf("first row %q", l)
	}
}
