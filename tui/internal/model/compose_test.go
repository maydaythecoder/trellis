package model

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// queueOf is the composer's list as the view words it, one item a line.
func queueOf(m *Model) []string {
	var out []string
	for _, q := range m.composeState().Queue {
		out = append(out, strings.TrimSpace(view.Plain(viewRowText(q))))
	}
	return out
}

// viewRowText draws one item alone, through the task box, and reads its row.
func viewRowText(q view.Queued) string {
	s := view.State{Compose: view.Compose{Queue: []view.Queued{q}}}
	rows := strings.Split(view.Render(s, 120, 40), "\n")
	for i, r := range rows {
		if strings.Contains(r, "█") {
			return strings.Trim(rows[i+1], "│ ")
		}
	}
	return ""
}

// pendingAnswer answers the get_state in out, the last one, with pending.
func pendingAnswer(t *testing.T, m *Model, out []Out, pending []any) {
	t.Helper()
	for i := len(out) - 1; i >= 0; i-- {
		if out[i].Cmd["type"] == "get_state" {
			respond(m, out[i].Cmd["id"], "get_state", map[string]any{"pending": pending})
			return
		}
	}
	t.Fatalf("no get_state was sent: %v", kinds(out))
}

func runStart(id any) string { return jsonLine(map[string]any{"type": "run_start", "id": id}) }

// The risk (#852, #825): Queue sending a steer, which the run in flight
// would read as an instruction to itself, or a prompt, which rpc refuses
// mid-run; or the queue drawing an item trellis never listed. Queue sends
// follow_up with the line, then asks get_state, whose pending alone puts
// the item in the list; its own run_start marks it delivered, with no turn,
// since its run follows none of the one before. Once no run is in flight,
// the same line is a task again.
func TestComposeQueueSendsFollowUp(t *testing.T) {
	m := running(t)
	typeLine(m, "then rename it")
	tabKey(m)
	enter(m)
	out := m.TakeOutbox()
	if got := strings.Join(kinds(out), ","); got != "follow_up,get_state" {
		t.Fatalf("Queue sent %s, want follow_up then get_state", got)
	}
	if out[0].Cmd["message"] != "then rename it" || m.Input != "" {
		t.Fatalf("follow_up %v, input %q", out[0].Cmd, m.Input)
	}
	if len(queueOf(m)) != 0 {
		t.Fatalf("listed before get_state named it: %q", queueOf(m))
	}
	id := out[0].Cmd["id"]
	pendingAnswer(t, m, out, []any{
		map[string]any{"id": id, "type": "follow_up", "message": "then rename it\x1b[31m"},
		// Not this client's (no id), and a type the contract does not list.
		map[string]any{"id": nil, "type": "follow_up", "message": "someone else's"},
		map[string]any{"id": "x", "type": "prompt", "message": "not a pending kind"},
	})
	// Served text is made plain: the escape byte goes.
	if got := queueOf(m); len(got) != 1 || got[0] != "queued · follow_up: then rename it[31m" {
		t.Fatalf("queue %q", got)
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if got := queueOf(m); len(got) != 1 || got[0] != "queued · follow_up: then rename it[31m" {
		t.Fatalf("after run_end, before its own run: %q", got)
	}
	feed(m, runStart(id))
	if got := queueOf(m); len(got) != 1 || got[0] != "delivered · follow_up: then rename it[31m" {
		t.Fatalf("after its own run_start: %q", got)
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	m.TakeOutbox()
	typeLine(m, "next task")
	enter(m)
	if got := kinds(m.TakeOutbox()); strings.Join(got, ",") != "classify" {
		t.Fatalf("with no run in flight the line sent %v, want it classified as a task", got)
	}
}

// The risk (#852, DESIGN.md section 2.1): the held prompt sent at run_end
// alone, which rpc writes before it answers the abort, so a refused stop
// would be followed by a second run against a tree the first still holds;
// or sent after a refused abort, an unrelated abort (^C's) or the child's
// exit. Stop and send sends abort and holds the line; the prompt goes only
// once run_end and that abort's successful answer have both been read,
// and its own run_start marks it delivered. One line is held at a time.
func TestComposeStopAndSendAbortsThenPrompts(t *testing.T) {
	m := running(t)
	typeLine(m, "start over")
	tabKey(m)
	tabKey(m)
	enter(m)
	out := m.TakeOutbox()
	if got := strings.Join(kinds(out), ","); got != "abort" {
		t.Fatalf("Stop and send sent %s, want abort alone", got)
	}
	abortID := out[0].Cmd["id"]
	if got := queueOf(m); len(got) != 1 || got[0] != "queued · prompt: start over" {
		t.Fatalf("queue %q", got)
	}
	// The verb is still Stop and send.
	typeLine(m, "another")
	enter(m)
	if out := m.TakeOutbox(); len(out) != 0 || m.Input != "another" {
		t.Fatalf("a second Stop and send sent %v, input %q; one line is held at a time", kinds(out), m.Input)
	}
	m.Input = ""

	feed(m, runEnd(map[string]any{"outcome": "interrupted", "restored": true}))
	if got := kinds(m.TakeOutbox()); strings.Contains(strings.Join(got, ","), "prompt") {
		t.Fatalf("sent %v at run_end, before the abort was answered", got)
	}
	// A line typed now would race the held one.
	typeLine(m, "meanwhile")
	enter(m)
	if out := m.TakeOutbox(); len(out) != 0 || m.Input != "meanwhile" {
		t.Fatalf("a task while the line is held sent %v", kinds(out))
	}
	m.Input = ""
	respond(m, "tui-999", "abort", map[string]any{"undelivered": []any{}})
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("another abort's answer sent %v", kinds(out))
	}
	respond(m, abortID, "abort", map[string]any{"undelivered": []any{}})
	out = m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "prompt" || out[0].Cmd["task"] != "start over" || !m.Running {
		t.Fatalf("after run_end and the abort's answer sent %v, running %v", out, m.Running)
	}
	feed(m, runStart(out[0].Cmd["id"]))
	if got := queueOf(m); len(got) != 1 || got[0] != "delivered · prompt: start over" {
		t.Fatalf("after its run_start: %q", got)
	}

	// Refused: the run goes on, and the line is never sent.
	m = running(t)
	typeLine(m, "start over")
	tabKey(m)
	tabKey(m)
	enter(m)
	abortID = m.TakeOutbox()[0].Cmd["id"]
	feed(m, jsonLine(map[string]any{"type": "response", "command": "abort", "id": abortID, "success": false, "error": "no run in flight"}))
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if got := kinds(m.TakeOutbox()); strings.Contains(strings.Join(got, ","), "prompt") {
		t.Fatalf("a refused stop sent %v", got)
	}
	if got := queueOf(m); len(got) != 1 || got[0] != "not delivered (the stop was refused) · prompt: start over" {
		t.Fatalf("queue after a refused stop: %q", got)
	}
	if !strings.Contains(lines(m), "· not delivered: start over (the stop was refused)") {
		t.Fatalf("the transcript does not say it:\n%s", lines(m))
	}

	// The child exits before the run ends: nothing is sent.
	m = running(t)
	typeLine(m, "start over")
	tabKey(m)
	tabKey(m)
	enter(m)
	m.TakeOutbox()
	m.Update(ExitedMsg{})
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("sent %v after the child exited", kinds(out))
	}
	if got := queueOf(m); len(got) != 1 || !strings.HasPrefix(got[0], "not delivered (trellis rpc exited") {
		t.Fatalf("queue after the exit: %q", got)
	}
}

// The risk (#852, #851): Tab taken from the panes or from @ completion,
// or Stop and send left chosen for the next run's line. While a run is in
// flight a line with text takes Tab for the verb, Steer, Queue, Stop and
// send and round again, and Shift-Tab goes back; an empty line keeps Tab
// for the panes; the title names the verb and the bottom border the next.
// The verb is Steer again whenever a run starts or ends.
func TestComposeTabCyclesVerbsWhileRunning(t *testing.T) {
	m := running(t)
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	tabKey(m)
	if m.verb != view.VerbSteer || m.nav.Selected == "" {
		t.Fatalf("Tab on an empty line: verb %v, selected %q; want a pane selected", m.verb, m.nav.Selected)
	}
	escKey(m)
	typeLine(m, "x")
	for _, want := range []view.Verb{view.VerbQueue, view.VerbStop, view.VerbSteer, view.VerbQueue} {
		tabKey(m)
		if m.verb != want {
			t.Fatalf("Tab: verb %v, want %v", m.verb, want)
		}
	}
	shiftTabKey(m)
	shiftTabKey(m)
	if m.verb != view.VerbStop {
		t.Fatalf("Shift-Tab: verb %v, want stop and send", m.verb)
	}
	if v := m.View(); !strings.Contains(v, "stop and send  [⏎] stop this run (rolls back), then run this") || !strings.Contains(v, "[TAB] steer") {
		t.Fatalf("the task box does not name the verb and the next:\n%s", v)
	}
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("choosing a verb sent %v", kinds(out))
	}

	typeLine(m, " @mo")
	tabKey(m)
	if m.verb != view.VerbStop {
		t.Fatalf("Tab after an @ word changed the verb to %v", m.verb)
	}

	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if m.verb != view.VerbSteer {
		t.Fatalf("after run_end the verb is %v, want steer", m.verb)
	}
	m.Input = "y"
	selected := m.nav.Selected
	tabKey(m)
	if m.verb != view.VerbSteer || m.nav.Selected == selected {
		t.Fatalf("Tab with no run: verb %v, selected %q; want the panes", m.verb, m.nav.Selected)
	}

	m = running(t)
	typeLine(m, "x")
	tabKey(m)
	tabKey(m)
	feed(m, runStart("tui-77"))
	if m.verb != view.VerbSteer {
		t.Fatalf("after run_start the verb is %v, want steer", m.verb)
	}
}

// The risk (#808): a steer marked delivered because a later get_state no
// longer lists it, which is true of it before its delivered event is
// written, and of one the run ended before. Only an event settles an
// item: the steer delivered event, with the turn the request followed; a
// run that ends first; and an abort's undelivered.
func TestComposeQueueItemsChangeOnlyOnEvents(t *testing.T) {
	m := running(t)
	for _, s := range []string{"first", "second"} {
		typeLine(m, s)
		enter(m)
	}
	out := m.TakeOutbox()
	pendingAnswer(t, m, out, []any{
		map[string]any{"id": out[0].Cmd["id"], "type": "steer", "message": "first"},
		map[string]any{"id": out[2].Cmd["id"], "type": "steer", "message": "second"},
	})
	pendingAnswer(t, m, out, []any{})
	if got := queueOf(m); len(got) != 2 || got[0] != "queued · steer: first" || got[1] != "queued · steer: second" {
		t.Fatalf("an item missing from pending moved: %q", got)
	}
	feed(m, steerDelivered(2))
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	got := queueOf(m)
	if len(got) != 2 || got[0] != "delivered after turn 2 · steer: first" || got[1] != "not delivered (the run ended first) · steer: second" {
		t.Fatalf("queue %q", got)
	}
	// The next run starts with what the last one settled gone.
	submit(m, "next")
	feed(m, runStart(m.promptID))
	if got := queueOf(m); len(got) != 0 {
		t.Fatalf("settled items kept into the next run: %q", got)
	}
}
