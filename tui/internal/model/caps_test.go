package model

import (
	"fmt"
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// #848: a run's actions, edits and checks keep the newest PaneCap each, and
// every pane says how many older rows it no longer holds; the bar still
// counts every write while the run is in flight, and once it has ended
// says "+" of the files it can no longer count.
func TestCapsActionsEditsAndChecksHold(t *testing.T) {
	m := running(t)
	const over = 5
	for i := 1; i <= PaneCap+over; i++ {
		feed(m, turn(i, fmt.Sprintf("edit app/f%d.py", i), map[string]any{
			"observation": "applied: true", "verdict": "Some(true)", "check_kind": "pytest",
		}))
	}
	for name, got := range map[string][2]int{
		"actions": {m.Actions.Len(), m.Actions.Dropped()},
		"edits":   {m.Edits.Len(), m.Edits.Dropped()},
		"checks":  {m.Checks.Len(), m.Checks.Dropped()},
	} {
		if got != [2]int{PaneCap, over} {
			t.Fatalf("%s: %d kept, %d dropped; want %d and %d", name, got[0], got[1], PaneCap, over)
		}
	}
	if first := m.Edits.Slice()[0].Path; first != fmt.Sprintf("app/f%d.py", over+1) {
		t.Fatalf("the oldest edit kept is %s: the newest are kept, not the first", first)
	}
	m.Update(windowSize(140, 60))
	v := m.View()
	for _, want := range []string{
		fmt.Sprintf("● %d pending", PaneCap+over),
		fmt.Sprintf("… %d earlier edits not kept", over),
		fmt.Sprintf("… %d earlier checks not kept", over),
		fmt.Sprintf("%d earlier actions not kept", over),
	} {
		if !strings.Contains(v, want) {
			t.Errorf("%q is not on screen:\n%s", want, v)
		}
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if v := m.View(); !strings.Contains(v, fmt.Sprintf("● %d+ to review", PaneCap)) {
		t.Errorf("after the run the bar does not say it cannot count every file:\n%s", v)
	}
}

// #848: questions waiting behind the one on screen stop at ApprovalsCap;
// one more is declined at once, said so, and never left unanswered, since
// trellis would wait on it for good. A run's steers stop at SteersCap; the
// next is not sent and stays on the task line, so nothing typed is lost.
func TestCapsPendingApprovalsAndSteersHold(t *testing.T) {
	m := running(t)
	for i := 0; i <= ApprovalsCap+1; i++ {
		feed(m, approval(fmt.Sprintf("a%d", i), fmt.Sprintf("edit f%d.py", i), ""))
	}
	if len(m.approvals) != ApprovalsCap {
		t.Fatalf("%d questions waiting, want %d", len(m.approvals), ApprovalsCap)
	}
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "approve" || out[0].Cmd["yes"] != false || out[0].Cmd["id"] != fmt.Sprintf("a%d", ApprovalsCap+1) {
		t.Fatalf("the question past the cap was not declined: sent %+v", out)
	}
	if want := fmt.Sprintf("· edit f%d.py declined unseen: %d questions were already waiting", ApprovalsCap+1, ApprovalsCap+1); last(m) != want {
		t.Fatalf("last line %q, want %q", last(m), want)
	}

	m = running(t)
	for i := 0; i < SteersCap; i++ {
		typeLine(m, fmt.Sprintf("s%d", i))
		enter(m)
	}
	m.TakeOutbox()
	typeLine(m, "one more")
	enter(m)
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("a steer past the cap was sent: %+v", out)
	}
	if len(m.steers) != SteersCap || m.Input != "one more" || !strings.HasPrefix(last(m), "· not sent: this run has") {
		t.Fatalf("%d steers, input %q, last %q", len(m.steers), m.Input, last(m))
	}
}

// #848: the answers still awaited are the newest UnansweredCap. A child
// that stops answering cannot grow the map; an answer to a forgotten
// command is ignored; and a forgotten classify ends the task line's wait,
// rather than leaving it asking for good.
func TestCapsUnansweredCommandsHold(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix the parser")
	enter(m)
	classify := m.TakeOutbox()[0].Cmd["id"]
	for i := 0; i < 3*UnansweredCap; i++ {
		m.refreshState()
	}
	if len(m.purposes) != UnansweredCap || m.asked.Len() != UnansweredCap {
		t.Fatalf("%d answers awaited, %d ids held; want %d", len(m.purposes), m.asked.Len(), UnansweredCap)
	}
	// Asked: the get_state New sends, the classify, and the refreshes;
	// none was answered, so all but the newest UnansweredCap are forgotten.
	if want := 2 + 3*UnansweredCap - UnansweredCap; m.forgotten != want {
		t.Fatalf("%d forgotten, want %d", m.forgotten, want)
	}
	if m.classifying || !strings.Contains(strings.Join(texts(m), "\n"), "never said whether that was a question or a task") {
		t.Fatalf("a forgotten classify left the task line waiting: classifying %v\n%s", m.classifying, strings.Join(texts(m), "\n"))
	}
	m.TakeOutbox()
	respond(m, classify, "classify", map[string]any{"intent": "task"})
	if out := m.TakeOutbox(); len(out) != 0 || m.Running {
		t.Fatalf("an answer to a forgotten classify started a run: %+v", out)
	}
}

// #848: a transcript line past LineCap bytes is cut at a character's
// start and says how much was cut; every frame wraps every line again, so
// one long line would cost every key press.
func TestCapsLongTranscriptLineIsCutAndSaysSo(t *testing.T) {
	m := fresh(t)
	long := strings.Repeat("é", LineCap) // two bytes each
	m.push(view.RoleText, long)
	got := last(m)
	if !utf8.ValidString(got) {
		t.Fatalf("the cut split a character: %q", got[len(got)-40:])
	}
	kept, said, ok := strings.Cut(got, "… [")
	if !ok || len(kept) > LineCap || said != fmt.Sprintf("%d more bytes cut]", len(long)-len(kept)) {
		t.Fatalf("kept %d bytes, then %q", len(kept), said)
	}
	short := strings.Repeat("x", LineCap)
	m.push(view.RoleText, short)
	if last(m) != short {
		t.Fatal("a line of exactly LineCap bytes was cut")
	}
}
