package model

import (
	"errors"
	"fmt"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

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

// screenHas reports whether the screen m draws holds text.
func screenHas(m *Model, text string) bool { return strings.Contains(m.View(), text) }

// logRun is a run with a row of each group: the person's task, a read
// that wrote nothing and what it returned, a check, an edit that wrote, a
// note, the model's question, a refusal and a line that is no frame, the
// model's own words.
func logRun(t *testing.T) *Model {
	t.Helper()
	m := running(t)
	m.Update(windowSize(120, 40))
	feed(m, turn(1, "read_file a.py", map[string]any{"tool": "read_file", "target": "a.py", "observation": "def f():\n    return 1"}))
	feed(m, turn(2, "run_checks", map[string]any{"tool": "run_checks", "target": "all", "verdict": "Some(false)", "check_kind": "tests", "observation": "1 failed"}))
	feed(m, turn(3, "edit f.py", map[string]any{"tool": "edit", "target": "f.py", "wrote": true, "observation": "applied the edit"}))
	feed(m, warn("the model is slow"))
	feed(m, approval("a1", `ask_human "Keep the v1 endpoint?"`, ""))
	m.Update(keyType(tea.KeyEsc))
	m.Modal, m.approvals = nil, nil
	m.Update(SendFailedMsg{Err: errors.New("broken pipe")})
	feed(m, "the model's own words")
	m.TakeOutbox()
	return m
}

// The risks (#948): a filter that drops the rows it hides, so switching
// back loses them; a group that is not the desktop's (#949), or one that
// hides the turn dividers its rows read under; the scroll limits counted
// over every row while fewer are drawn, so Home and PgUp spent presses on
// rows not there (#843); and a key that sends trellis anything.
func TestActionLogFilterHidesRowsNeverDropsThem(t *testing.T) {
	m := logRun(t)
	all := strings.Join(texts(m), "\n")
	rows := map[string]string{
		"task":    "you · task  change f",
		"read":    "· Read a.py",
		"output":  "    return 1",
		"check":   "✗ tests failed · trellis-checks verdict",
		"edit":    "· Edited f.py",
		"note":    "the model is slow",
		"ask":     "? model asks you",
		"refusal": "! could not write to trellis rpc",
		"text":    "the model's own words",
	}
	for name, text := range rows {
		if !strings.Contains(all, text) {
			t.Fatalf("the run wrote no %s row %q:\n%s", name, text, all)
		}
	}
	dividers := []string{"─ turn 1 ", "─ turn 2 ", "─ turn 3 "}
	shown := map[view.LogFilter][]string{
		view.FilterYou:    {"task", "ask"},
		view.FilterTools:  {"read", "output", "edit", "refusal"},
		view.FilterChecks: {"check"},
		view.FilterNotes:  {"note", "ask", "refusal"},
		view.FilterAll:    {"task", "read", "output", "check", "edit", "note", "ask", "refusal", "text"},
	}
	held := len(m.Transcript)
	// ^F visits You, Tools, Checks, Notes, then All again.
	for _, f := range []view.LogFilter{view.FilterYou, view.FilterTools, view.FilterChecks, view.FilterNotes, view.FilterAll} {
		ctrl(m, tea.KeyCtrlF)
		if m.filter != f {
			t.Fatalf("^F went to %q, want %q", m.filter, f)
		}
		if len(m.Transcript) != held || strings.Join(texts(m), "\n") != all {
			t.Fatalf("under %q the transcript holds %d rows, was %d", f, len(m.Transcript), held)
		}
		want := map[string]bool{}
		for _, name := range shown[f] {
			want[name] = true
		}
		for name, text := range rows {
			if got := screenHas(m, text); got != want[name] {
				t.Errorf("under %q the %s row drawn: %v, want %v", f, name, got, want[name])
			}
		}
		for _, d := range dividers {
			if !screenHas(m, d) {
				t.Errorf("under %q the divider %q is not drawn", f, d)
			}
		}
		if out := m.TakeOutbox(); len(out) != 0 {
			t.Fatalf("^F sent %v", kinds(out))
		}
	}
	if strings.Contains(m.View(), "hidden") {
		t.Fatal("back at All the title still says rows are hidden")
	}

	// The scroll limits count the rows drawn. Under Checks the few drawn
	// fit on one page, so there is nowhere above to go; under All they do
	// not, and the top is the rows less a page.
	for i := 0; i < 60; i++ {
		feed(m, warn("filler"))
	}
	m.Update(keyType(tea.KeyHome))
	if m.ScrollBack == 0 || m.ScrollBack != m.transcriptTop() {
		t.Fatalf("under All, Home went to %d of top %d", m.ScrollBack, m.transcriptTop())
	}
	for m.filter != view.FilterChecks {
		ctrl(m, tea.KeyCtrlF)
	}
	if m.ScrollBack != 0 || m.transcriptTop() != 0 {
		t.Fatalf("under Checks the scroll back is %d and the top %d, with every drawn row on one page", m.ScrollBack, m.transcriptTop())
	}
	if !screenHas(m, "transcript · checks · ") || !screenHas(m, " hidden") {
		t.Fatalf("the title does not say the filter hides rows:\n%s", m.View())
	}
	// Both keys are on the status bar and in /help.
	last := strings.Split(m.View(), "\n")
	if bar := last[len(last)-1]; !strings.Contains(bar, "[^F] filter") || !strings.Contains(bar, "[^N] quiet") {
		t.Fatalf("the status bar reads %q", bar)
	}
	for ctrl(m, tea.KeyCtrlF); m.filter != view.FilterAll; ctrl(m, tea.KeyCtrlF) {
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	typeLine(m, "/help")
	enter(m)
	help := strings.Join(texts(m), "\n")
	for _, k := range []string{"[^F]", "[^N]", "[^O]"} {
		if !strings.Contains(help, k) {
			t.Errorf("/help does not list %s", k)
		}
	}
}

// The risks (#948, the desktop's #949): quiet hiding a call that wrote,
// which the trail and the Edits pane count as an edit (#800), or a check,
// a note, the person's words or a divider; and quiet leaving the
// observation lines it is for. Quiet hides every call that wrote nothing
// and every observation line, and ^N again draws them all.
func TestActionLogQuietHidesObservations(t *testing.T) {
	m := logRun(t)
	all := strings.Join(texts(m), "\n")
	ctrl(m, tea.KeyCtrlN)
	if !m.quiet || strings.Join(texts(m), "\n") != all {
		t.Fatalf("quiet %v changed the transcript", m.quiet)
	}
	for _, gone := range []string{"def f():", "    return 1", "1 failed", "applied the edit", "· Read a.py", "· Ran the checks: all"} {
		if screenHas(m, gone) {
			t.Errorf("quiet still draws %q:\n%s", gone, m.View())
		}
	}
	for _, kept := range []string{"· Edited f.py", "✗ tests failed · trellis-checks verdict", "you · task  change f",
		"the model is slow", "? model asks you", "! could not write to trellis rpc", "─ turn 1 ", "─ turn 2 ", "─ turn 3 ", "transcript · quiet · "} {
		if !screenHas(m, kept) {
			t.Errorf("quiet does not draw %q:\n%s", kept, m.View())
		}
	}
	for _, r := range m.Transcript {
		if r.Role == view.RoleOutput && view.Shows(r, view.FilterAll, true) {
			t.Errorf("an observation line shown in quiet: %+v", r)
		}
	}
	// Quiet holds under a filter: Tools draws only the call that wrote.
	ctrl(m, tea.KeyCtrlF)
	ctrl(m, tea.KeyCtrlF)
	if m.filter != view.FilterTools || screenHas(m, "· Read a.py") || !screenHas(m, "· Edited f.py") {
		t.Fatalf("Tools under quiet:\n%s", m.View())
	}
	ctrl(m, tea.KeyCtrlN)
	for _, back := range []string{"· Read a.py", "    return 1", "applied the edit"} {
		if !screenHas(m, back) {
			t.Errorf("^N again does not draw %q", back)
		}
	}
}

// The risks (#948): ^O opening a turn other than the one the person is
// looking at; the call or observation reaching the screen with its
// escapes; and a call box that does not show what the six lines under the
// call left out. ^O opens the newest turn the transcript draws, in full,
// made plain, ESC closes it and nothing is sent.
func TestActionLogRawCallOpensTheTurnOnScreen(t *testing.T) {
	m := running(t)
	m.Update(windowSize(120, 40))
	var obs []string
	for i := 1; i <= 9; i++ {
		obs = append(obs, fmt.Sprintf("line %d of a.py", i))
	}
	feed(m, turn(1, `{"tool":"read_file","path":"a.py"}`+"\x1b[2J", map[string]any{"tool": "read_file", "target": "a.py", "observation": strings.Join(obs, "\n")}))
	for i := 0; i < 60; i++ {
		feed(m, warn("filler"))
	}
	feed(m, turn(2, `{"tool":"grep","pattern":"f"}`, map[string]any{"tool": "grep", "target": "f"}))
	m.TakeOutbox()

	ctrl(m, tea.KeyCtrlO)
	if m.call == nil || m.call.raw.Turn != 2 {
		t.Fatalf("following the end, ^O opened %+v", m.call)
	}
	if !screenHas(m, `{"tool":"grep","pattern":"f"}`) || !screenHas(m, "trellis sent no observation for it") {
		t.Fatalf("the call box:\n%s", m.View())
	}
	m.Update(keyType(tea.KeyEsc))
	if m.call != nil {
		t.Fatal("ESC left the call box open")
	}

	// Scrolled to the start, turn 1 is the newest turn on screen.
	m.Update(keyType(tea.KeyHome))
	ctrl(m, tea.KeyCtrlO)
	if m.call == nil || m.call.raw.Turn != 1 {
		t.Fatalf("at the start, ^O opened %+v", m.call)
	}
	screen := m.View()
	if strings.Contains(screen, "\x1b[2J") || !strings.Contains(screen, `{"tool":"read_file","path":"a.py"}[2J`) {
		t.Fatalf("the call is not drawn plain:\n%s", screen)
	}
	// Every line, the three the transcript's six left out among them.
	for _, l := range obs {
		if !strings.Contains(screen, l) {
			t.Errorf("the call box does not draw %q", l)
		}
	}
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("^O sent %v", kinds(out))
	}
	m.Update(keyType(tea.KeyEsc))

	// A turn's observation is kept to RawCap, and the cut is said.
	feed(m, turn(3, "bash", map[string]any{"tool": "bash", "target": "cat big", "observation": strings.Repeat("x", RawCap+10)}))
	r := m.Transcript[len(m.Transcript)-1].Raw
	if r == nil || len(*r.Observation) > RawCap+64 || !strings.HasSuffix(*r.Observation, "… [10 more bytes cut]") {
		t.Fatalf("the kept observation is %d bytes", len(*r.Observation))
	}
	// Under You no turn's call is drawn: ^O opens nothing and says why.
	ctrl(m, tea.KeyCtrlF)
	m.Update(keyType(tea.KeyEnd))
	ctrl(m, tea.KeyCtrlO)
	if m.call != nil || !strings.HasPrefix(last(m), "· no turn's call is in the transcript on screen") {
		t.Fatalf("under You ^O opened %+v, last row %q", m.call, last(m))
	}
}
