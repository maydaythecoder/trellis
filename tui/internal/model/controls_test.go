package model

import (
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

func ctrl(m *Model, k tea.KeyType) { m.Update(tea.KeyMsg{Type: k}) }

// refuse answers id's command with success false and why.
func refuse(m *Model, id any, command, why string) {
	feed(m, jsonLine(map[string]any{"type": "response", "command": command, "id": id, "success": false, "error": why}))
}

func event(message string, fields map[string]any) string {
	f := map[string]any{"message": message}
	for k, v := range fields {
		f[k] = v
	}
	return jsonLine(map[string]any{"level": "INFO", "fields": f})
}

// clearLine empties the task line a key at a time, as a person would.
func clearLine(m *Model) {
	for m.Input != "" {
		press(m, tea.KeyBackspace)
	}
}

func screen(m *Model) string {
	m.Update(windowSize(120, 40))
	return m.View()
}

// The risk (#857, #806, the desktop's #837): the strip says PAUSED on the
// key, before the run is held, or keeps saying it after the engine's
// resumed event, so a person reads a run that is moving as held or one
// that is held as moving; or this view's clock counts the hold, which
// trellis's elapsed_ms and its wall-clock budget leave out. The word comes
// from the paused event alone and goes on resumed or the run's end; a
// pause asked and not yet reached already offers resume, which withdraws
// it; the clock stops for the hold.
func TestPauseAndResumeFollowTheEnginesEvents(t *testing.T) {
	idle := fresh(t)
	ctrl(idle, tea.KeyCtrlP)
	if out := idle.TakeOutbox(); len(out) != 0 || strings.Contains(stripOf(t, idle), "^P") {
		t.Fatalf("with no run ^P sent %v, or the strip offers it: %q", kinds(out), stripOf(t, idle))
	}

	m := fresh(t)
	clock := time.Date(2026, 10, 26, 7, 0, 0, 0, time.UTC)
	m.now = func() time.Time { return clock }
	submit(m, "change f")
	m.TakeOutbox()
	feed(m, turn(1, "read_file a.py", map[string]any{"tool": "read_file", "target": "a.py", "elapsed_ms": 10000}))
	if s := stripOf(t, m); !strings.Contains(s, "● RUNNING") || !strings.Contains(s, "[^P] pause") || !strings.Contains(s, "0:10") {
		t.Fatalf("running: %q", s)
	}

	ctrl(m, tea.KeyCtrlP)
	out := m.TakeOutbox()
	if got := strings.Join(kinds(out), ","); got != "pause" {
		t.Fatalf("^P sent %s, want pause", got)
	}
	pauseID := out[0].Cmd["id"]
	if s := stripOf(t, m); !strings.Contains(s, "● RUNNING") || strings.Contains(s, "PAUSED") || !strings.Contains(s, "[^P] resume") {
		t.Fatalf("a pause asked is not yet a hold, and resume withdraws it: %q", s)
	}
	clock = clock.Add(5 * time.Second)
	m.Update(TickMsg{})
	if s := stripOf(t, m); !strings.Contains(s, "0:15") {
		t.Fatalf("before the hold the clock runs: %q", s)
	}

	feed(m, event("paused", map[string]any{"turn": 2}))
	if s := stripOf(t, m); !strings.Contains(s, "● PAUSED") || !strings.Contains(s, "[^P] resume") {
		t.Fatalf("after the paused event: %q", s)
	}
	if v := screen(m); !strings.Contains(v, "phase    paused before the next model call") {
		t.Fatalf("Where it is does not say the run is held:\n%s", v)
	}
	clock = clock.Add(60 * time.Second)
	m.Update(TickMsg{})
	if s := stripOf(t, m); !strings.Contains(s, "0:15") {
		t.Fatalf("the clock counted the hold: %q", s)
	}
	respond(m, pauseID, "pause", map[string]any{"turn": 2})
	ctrl(m, tea.KeyCtrlP)
	if got := strings.Join(kinds(m.TakeOutbox()), ","); got != "resume" {
		t.Fatalf("^P while held sent %s, want resume", got)
	}
	if s := stripOf(t, m); !strings.Contains(s, "● PAUSED") {
		t.Fatalf("a resume sent is not the run going on; the resumed event is: %q", s)
	}
	clock = clock.Add(30 * time.Second)
	feed(m, event("resumed", map[string]any{"turn": 2, "paused_ms": 90000}))
	if s := stripOf(t, m); !strings.Contains(s, "● RUNNING") || !strings.Contains(s, "[^P] pause") || strings.Contains(s, "PAUSED") {
		t.Fatalf("after the resumed event: %q", s)
	}
	if !strings.Contains(last(m), "resumed after a hold of 90.0 s") {
		t.Fatalf("the hold trellis measured is not said: %q", last(m))
	}
	clock = clock.Add(3 * time.Second)
	m.Update(TickMsg{})
	if s := stripOf(t, m); !strings.Contains(s, "0:18") {
		t.Fatalf("after the hold the clock goes on from where it stopped: %q", s)
	}

	// A pause asked and withdrawn before the hold: trellis refuses the
	// pause, saying so, and the strip offers pause again.
	ctrl(m, tea.KeyCtrlP)
	ctrl(m, tea.KeyCtrlP)
	out = m.TakeOutbox()
	if got := strings.Join(kinds(out), ","); got != "pause,resume" {
		t.Fatalf("pause then ^P sent %s, want pause,resume", got)
	}
	refuse(m, out[0].Cmd["id"], "pause", "withdrawn by a resume before the hold was reached")
	if s := stripOf(t, m); !strings.Contains(s, "● RUNNING") || !strings.Contains(s, "[^P] pause") {
		t.Fatalf("a refused pause still offers resume: %q", s)
	}
	if !strings.Contains(last(m), "! pause: withdrawn by a resume") {
		t.Fatalf("the refusal is not said: %q", last(m))
	}

	// An abort ends a held run with no resumed event.
	ctrl(m, tea.KeyCtrlP)
	m.TakeOutbox()
	feed(m, event("paused", map[string]any{"turn": 3}))
	feed(m, runEnd(map[string]any{"outcome": "interrupted", "exit": 130, "restored": true}))
	if s := stripOf(t, m); strings.Contains(s, "PAUSED") || strings.Contains(s, "^P") || !strings.Contains(s, "ENDED · interrupted") {
		t.Fatalf("an ended run is still held: %q", s)
	}
	m.TakeOutbox()
	ctrl(m, tea.KeyCtrlP)
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("^P after the run sent %v", kinds(out))
	}

	// The child's exit ends a hold too.
	gone := running(t)
	feed(gone, event("paused", map[string]any{"turn": 1}))
	gone.Update(ExitedMsg{})
	if gone.paused || gone.pauseAsked || strings.Contains(stripOf(t, gone), "PAUSED") {
		t.Fatalf("a hold outlived the child: %q", stripOf(t, gone))
	}
}

// The risk (#857, #807, the desktop's #837): the budget row draws the
// ceiling the person typed, or set_budget's answer, before the run takes
// it, so the row claims a limit the run does not enforce; a raise is sent
// for the context, which the driver fixes, or for a figure that is not a
// whole number above the ceiling; the wall clock is sent in the seconds it
// is typed in; or the run budget sent again empties what the run has
// counted. The row's ceiling moves only with the run budget trellis sends
// again.
func TestRaiseSendsSetBudgetAndDrawsOnlyTheRunBudget(t *testing.T) {
	m := running(t)
	feed(m, event("run budget", map[string]any{"max_iterations": 10, "max_wall_clock_ms": 300000, "max_tokens": 5000}))
	feed(m, turn(3, "read_file a.py", map[string]any{"tool": "read_file", "target": "a.py", "elapsed_ms": 60000, "completion_tokens_used": 400}))
	typeLine(m, "keep this")
	if v := screen(m); !strings.Contains(v, "budget  [^R] raise") {
		t.Fatalf("the budget does not offer ^R:\n%s", v)
	}
	ctrl(m, tea.KeyCtrlR)
	v := screen(m)
	if m.Input != "" || !strings.Contains(v, "budget · raise turns") || !strings.Contains(v, "3/10") || !strings.Contains(v, "raise turns, in turns") {
		t.Fatalf("^R did not open a raise of the turns, the line %q:\n%s", m.Input, v)
	}
	for _, bad := range []string{"abc", "-5", "1.5", "0", "10", "4294967296"} {
		typeLine(m, bad)
		enter(m)
		if out := m.TakeOutbox(); len(out) != 0 || !strings.Contains(last(m), "not sent") {
			t.Fatalf("%q sent %v, or said nothing: %q", bad, kinds(out), last(m))
		}
		clearLine(m)
	}
	typeLine(m, "30")
	enter(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "set_budget" || out[0].Cmd["max_iterations"] != uint64(30) || len(out[0].Cmd) != 3 {
		t.Fatalf("raise sent %v", out)
	}
	if m.Input != "keep this" || m.raising != "" {
		t.Fatalf("the line was not given back: %q, raising %q", m.Input, m.raising)
	}
	respond(m, out[0].Cmd["id"], "set_budget", map[string]any{"max_iterations": 30, "max_wall_clock_ms": 300000, "max_tokens": 5000})
	if v := screen(m); !strings.Contains(v, "TURNS ████░░░░░░░░░░░░ 3/10") || strings.Contains(v, "/30") {
		t.Fatalf("the ceiling moved before the run budget said so:\n%s", v)
	}
	feed(m, event("run budget", map[string]any{"max_iterations": 30, "max_wall_clock_ms": 300000, "max_tokens": 5000}))
	if v := screen(m); !strings.Contains(v, "TURNS █░░░░░░░░░░░░░░░ 3/30") || !strings.Contains(v, "TIME 1:00") {
		t.Fatalf("the run budget sent again did not move the ceiling, or emptied what was counted:\n%s", v)
	}

	// The wall clock is typed in seconds and sent in milliseconds.
	ctrl(m, tea.KeyCtrlR)
	ctrl(m, tea.KeyCtrlR)
	typeLine(m, "300")
	enter(m)
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("a wall clock not above the ceiling's 300 s was sent: %v", out)
	}
	clearLine(m)
	typeLine(m, "600")
	enter(m)
	out = m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["max_wall_clock_ms"] != uint64(600000) {
		t.Fatalf("wall clock raise sent %v", out)
	}

	// Completion tokens, then nothing: there is no context row to raise,
	// and ESC gives the line back unsent.
	for _, want := range []view.Limit{view.LimitTurns, view.LimitWallClock, view.LimitTokens, ""} {
		ctrl(m, tea.KeyCtrlR)
		if m.raising != want {
			t.Fatalf("^R went to %q, want %q", m.raising, want)
		}
	}
	if m.Input != "keep this" {
		t.Fatalf("cycling past the last row lost the line: %q", m.Input)
	}
	ctrl(m, tea.KeyCtrlR)
	typeLine(m, "99")
	escKey(m)
	if out := m.TakeOutbox(); len(out) != 0 || m.raising != "" || m.Input != "keep this" {
		t.Fatalf("ESC sent %v, raising %q, line %q", kinds(out), m.raising, m.Input)
	}

	// A run's end closes a raise, and none is offered after it.
	ctrl(m, tea.KeyCtrlR)
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if m.raising != "" || m.Input != "keep this" {
		t.Fatalf("the raise outlived its run: %q, line %q", m.raising, m.Input)
	}
	ctrl(m, tea.KeyCtrlR)
	if m.raising != "" || strings.Contains(screen(m), "[^R]") {
		t.Fatalf("a raise with no run in flight")
	}
}

// The risk (#857, #808, the desktop's #838): the text goes back on the
// task line on the key, before trellis has withdrawn anything, so the
// person retypes a steer the model still reads; a withdraw names some id
// other than the item's own; or the item is said taken back when trellis
// refused. Only trellis's success answer takes it back, and its whole text
// joins the line after what is typed there, with the verb it was sent
// under; a refusal leaves it queued and says why; the steer's own refused
// answer, read first, does not keep it from being withdrawn.
func TestTakeBackWithdraws(t *testing.T) {
	m := running(t)
	typeLine(m, "use the helper")
	enter(m)
	steerID := m.TakeOutbox()[0].Cmd["id"]
	typeLine(m, "then rename it")
	tabKey(m)
	enter(m)
	out := m.TakeOutbox()
	followID := out[0].Cmd["id"]
	pendingAnswer(t, m, out, []any{
		map[string]any{"id": steerID, "type": "steer", "message": "use the helper"},
		map[string]any{"id": followID, "type": "follow_up", "message": "then rename it"},
	})
	if v := screen(m); !strings.Contains(v, "[^T] take back the newest queued") {
		t.Fatalf("^T is not offered:\n%s", v)
	}

	ctrl(m, tea.KeyCtrlT)
	out = m.TakeOutbox()
	if len(out) != 1 || len(out[0].Cmd) != 2 || out[0].Cmd["type"] != "withdraw" || out[0].Cmd["id"] != followID {
		t.Fatalf("^T sent %v, want withdraw with the follow-up's own id", out)
	}
	if m.Input != "" || queueOf(m)[1] != "queued, taking back · follow_up: then rename it" {
		t.Fatalf("on the key: line %q, queue %q", m.Input, queueOf(m))
	}
	ctrl(m, tea.KeyCtrlT)
	out = m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["id"] != steerID {
		t.Fatalf("the second ^T sent %v, want the steer's withdraw", out)
	}

	refuse(m, steerID, "withdraw", "already delivered")
	if q := queueOf(m); q[0] != "queued · steer: use the helper" || m.Input != "" || !strings.Contains(last(m), "! withdraw: already delivered") {
		t.Fatalf("a refused withdraw: queue %q, line %q, said %q", q, m.Input, last(m))
	}

	typeLine(m, "and")
	feed(m, jsonLine(map[string]any{"type": "response", "command": "withdraw", "id": followID, "success": true,
		"data": map[string]any{"id": followID, "type": "follow_up"}}))
	if q := queueOf(m); q[1] != "withdrawn · follow_up: then rename it" {
		t.Fatalf("queue %q", q)
	}
	if m.Input != "and then rename it" || m.composeState().Verb != view.VerbQueue {
		t.Fatalf("taken back: line %q, verb %v", m.Input, m.composeState().Verb)
	}

	// The steer: its own refused answer comes first, then the withdraw's
	// success.
	clearLine(m)
	ctrl(m, tea.KeyCtrlT)
	if out := m.TakeOutbox(); len(out) != 1 || out[0].Cmd["id"] != steerID {
		t.Fatalf("^T sent %v", out)
	}
	refuse(m, steerID, "steer", "withdrawn")
	feed(m, jsonLine(map[string]any{"type": "response", "command": "withdraw", "id": steerID, "success": true,
		"data": map[string]any{"id": steerID, "type": "steer"}}))
	if q := queueOf(m); q[0] != "withdrawn · steer: use the helper" || m.Input != "use the helper" || m.composeState().Verb != view.VerbSteer {
		t.Fatalf("queue %q, line %q, verb %v", q, m.Input, m.composeState().Verb)
	}
	ctrl(m, tea.KeyCtrlT)
	if out := m.TakeOutbox(); len(out) != 0 || !strings.Contains(last(m), "nothing to take back") {
		t.Fatalf("with nothing queued ^T sent %v, said %q", kinds(out), last(m))
	}
}

// The risk (#857, #810, the desktop's #838): an edit of the plan reaches
// the model as something other than an ordinary steer, or in other words
// than the desktop's, so the two clients tell the model different things;
// the plan drawn changes on the edit, though only the model's next plan
// event changes it; the model's words reach the terminal unplain; or a
// plan that is not the shape trellis keeps is drawn in part. An unchanged
// plan is not sent, and the task line comes back as it was.
func TestPlanEditIsSentAsASteer(t *testing.T) {
	m := running(t)
	if v := screen(m); !strings.Contains(v, "plan  summary so far") {
		t.Fatalf("the strip has no way into the plan:\n%s", v)
	}
	feed(m, event("plan", map[string]any{"turn": 4, "steps_cut": 0, "texts_cut": 0,
		"steps": `[{"text":"find the bug","state":"done"},{"text":"fix it\u001b[2J","state":"current"},{"text":"test it","state":"todo"}]`}))
	for range 6 {
		tabKey(m)
	}
	enter(m)
	typeLine(m, "half")
	v := screen(m)
	for _, want := range []string{"Run ⟩ plan", "[x] find the bug", "[>] fix it[2J", "[ ] test it", "[^E] edit"} {
		if !strings.Contains(v, want) {
			t.Fatalf("%q is not on the plan level:\n%s", want, v)
		}
	}
	if strings.Contains(v, "\x1b[2J") {
		t.Fatalf("the model's escape reached the screen")
	}

	ctrl(m, tea.KeyCtrlE)
	if m.Input != "[x] find the bug" {
		t.Fatalf("^E put %q on the line", m.Input)
	}
	enter(m)
	if out := m.TakeOutbox(); len(out) != 0 || !strings.Contains(last(m), "unchanged plan is not sent") {
		t.Fatalf("an unchanged plan sent %v, said %q", kinds(out), last(m))
	}
	press(m, tea.KeyDown)
	clearLine(m)
	typeLine(m, "[x] fix it")
	tabKey(m) // does nothing mid-edit: no verb, no pane
	press(m, tea.KeyDown)
	press(m, tea.KeyDown)
	typeLine(m, "[ ] add a test for the comment case")
	if v := screen(m); !strings.Contains(v, "▸ [ ] add a test for the comment case") || !strings.Contains(v, "plan step 4 of 4") {
		t.Fatalf("the edit is not drawn:\n%s", v)
	}
	enter(m)
	out := m.TakeOutbox()
	if got := strings.Join(kinds(out), ","); got != "steer,get_state" {
		t.Fatalf("the edit sent %s, want one steer then get_state", got)
	}
	want := "I edited your plan from turn 4. Work from this plan now, one step a line ([x] done, [>] current, [ ] to do):\n" +
		"[x] find the bug\n[x] fix it\n[ ] test it\n[ ] add a test for the comment case"
	if out[0].Cmd["message"] != want || len(out[0].Cmd) != 3 {
		t.Fatalf("steer %q, want %q", out[0].Cmd["message"], want)
	}
	if m.Input != "half" || m.planEdit != nil {
		t.Fatalf("the line was not given back: %q", m.Input)
	}
	if v := screen(m); !strings.Contains(v, "[>] fix it[2J") || strings.Contains(v, "add a test for the comment case") {
		t.Fatalf("the plan drawn changed on the edit:\n%s", v)
	}
	feed(m, steerDelivered(4))
	if !strings.Contains(last(m), "delivered after turn 4: I edited your plan from turn 4.") || !strings.Contains(last(m), "to do): [x] find the bug [x] fix it") {
		t.Fatalf("delivered: %q", last(m))
	}

	// A plan not in the shape trellis keeps is unreadable, whole, with why,
	// and has nothing to edit.
	for steps, why := range map[string]string{
		`not json`:                       "its steps are not JSON",
		`{"text":"a","state":"todo"}`:    "its steps are not a JSON array",
		`[{"text":"a","state":"maybe"}]`: "step 1's state is not todo, current or done",
		`[{"state":"todo"}]`:             "step 1 has no text",
		`[` + strings.Repeat(`{"text":"a","state":"todo"},`, 8) + `{"text":"a","state":"todo"}]`: "it carries 9 steps",
		`[{"text":"` + strings.Repeat("a", 201) + `","state":"todo"}]`:                           "step 1's text is longer than the 200 characters",
	} {
		feed(m, event("plan", map[string]any{"turn": 5, "steps": steps, "steps_cut": 0, "texts_cut": 0}))
		v := screen(m)
		if !strings.Contains(v, "the model's plan from turn 5 could not be read: "+why) || strings.Contains(v, "[ ] a") {
			t.Fatalf("%s: not refused whole with %q:\n%s", steps, why, v)
		}
		ctrl(m, tea.KeyCtrlE)
		if m.planEdit != nil || len(m.TakeOutbox()) != 0 {
			t.Fatalf("%s: an unreadable plan was opened for an edit", steps)
		}
	}

	// A run's end closes an edit, giving the line back, and none opens
	// after it.
	feed(m, event("plan", map[string]any{"turn": 6, "steps": `[{"text":"a","state":"todo"}]`, "steps_cut": 0, "texts_cut": 0}))
	ctrl(m, tea.KeyCtrlE)
	typeLine(m, " more")
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if m.planEdit != nil || m.Input != "half" {
		t.Fatalf("the edit outlived its run: line %q", m.Input)
	}
	ctrl(m, tea.KeyCtrlE)
	if m.planEdit != nil {
		t.Fatalf("an edit opened with no run in flight")
	}
}
