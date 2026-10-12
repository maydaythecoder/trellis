package model

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/rpc"
)

// fresh is a model past its start-up commands, with its outbox empty.
func fresh(t *testing.T) *Model {
	t.Helper()
	m := New(Options{})
	m.TakeOutbox()
	return m
}

func typeLine(m *Model, s string) {
	for _, r := range s {
		if r == ' ' {
			m.Update(tea.KeyMsg{Type: tea.KeySpace})
			continue
		}
		m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
	}
}

func enter(m *Model) { m.Update(tea.KeyMsg{Type: tea.KeyEnter}) }

// feed parses a line as the program does and applies it.
func feed(m *Model, line string) {
	m.Update(FrameMsg{Frame: frames.Parse(rpc.ParseLine(line))})
}

func jsonLine(v map[string]any) string {
	b, _ := json.Marshal(v)
	return string(b)
}

func turn(n int, call string, extra map[string]any) string {
	f := map[string]any{"message": "turn", "turn": n, "call": call}
	for k, v := range extra {
		f[k] = v
	}
	return jsonLine(map[string]any{"level": "INFO", "fields": f})
}

func runEnd(fields map[string]any) string {
	f := map[string]any{"type": "run_end"}
	for k, v := range fields {
		f[k] = v
	}
	return jsonLine(f)
}

func kinds(out []Out) []string {
	var k []string
	for _, o := range out {
		if o.Quit {
			k = append(k, "QUIT")
		} else {
			k = append(k, o.Cmd["type"].(string))
		}
	}
	return k
}

// A sent task becomes exactly one prompt line, carrying the text as typed,
// and the run is in flight until trellis says it ended.
func TestModelSentTaskBecomesOnePrompt(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix the bug")
	enter(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "prompt" || out[0].Cmd["task"] != "fix the bug" {
		t.Fatalf("outbox %+v", out)
	}
	if out[0].Cmd["id"] == nil {
		t.Fatal("the prompt has no id to match its run_end by")
	}
	if !m.Running || m.Input != "" || m.Transcript[len(m.Transcript)-1] != "> fix the bug" {
		t.Fatalf("running %v, input %q, transcript %q", m.Running, m.Input, m.Transcript)
	}
	// A second Enter while it runs starts no second run (it is a steer,
	// steer_test.go).
	typeLine(m, "again")
	enter(m)
	for _, o := range m.TakeOutbox() {
		if o.Cmd["type"] == "prompt" {
			t.Fatalf("a second prompt was sent mid-run: %+v", o)
		}
	}
}

// Typing builds the line, q is a letter like any other, backspace stops at
// empty, and an empty line submits nothing (app.rs's typing rows).
func TestModelTypingBackspaceAndAnEmptyLine(t *testing.T) {
	m := fresh(t)
	typeLine(m, "q")
	if m.Quitting || m.Input != "q" {
		t.Fatalf("q quit or was lost: quitting %v input %q", m.Quitting, m.Input)
	}
	for i := 0; i < 3; i++ {
		m.Update(tea.KeyMsg{Type: tea.KeyBackspace})
	}
	if m.Input != "" {
		t.Fatalf("input %q after backspaces", m.Input)
	}
	typeLine(m, "   ")
	enter(m)
	if out := m.TakeOutbox(); len(out) != 0 || m.Running {
		t.Fatalf("an empty line was sent: %+v", out)
	}
}

// Each turn frame adds one transcript line on its own, never on the end of
// the task echo, with the verdict trellis reported and nothing computed.
func TestModelTurnsFillTheTranscript(t *testing.T) {
	m := fresh(t)
	typeLine(m, "change f")
	enter(m)
	feed(m, turn(1, "read_file a.py", nil))
	feed(m, turn(2, "run_check tests", map[string]any{"verdict": "Some(true)"}))
	feed(m, turn(3, "run_check types", map[string]any{"verdict": "Some(false)"}))
	feed(m, turn(4, "edit a.py", map[string]any{"verdict": "None"}))
	want := []string{
		"> change f",
		"· read_file a.py",
		"· run_check tests  ✓ check passed",
		"· run_check types  ✗ check FAILED",
		"· edit a.py",
	}
	if strings.Join(m.Transcript, "\n") != strings.Join(want, "\n") {
		t.Fatalf("transcript:\n%s\nwant:\n%s", strings.Join(m.Transcript, "\n"), strings.Join(want, "\n"))
	}
}

// The transcript keeps the newest TranscriptCap lines, as the Rust TUI
// does: a long run would otherwise grow it without bound.
func TestModelTranscriptCapHolds(t *testing.T) {
	m := fresh(t)
	for i := 1; i <= TranscriptCap+500; i++ {
		feed(m, turn(i, fmt.Sprintf("step %d", i), nil))
	}
	if len(m.Transcript) != TranscriptCap {
		t.Fatalf("%d lines kept, want %d", len(m.Transcript), TranscriptCap)
	}
	if m.Transcript[0] != "· step 501" || m.Transcript[TranscriptCap-1] != fmt.Sprintf("· step %d", TranscriptCap+500) {
		t.Fatalf("kept %q .. %q", m.Transcript[0], m.Transcript[TranscriptCap-1])
	}
}

// run_end sets the outcome, shown as "last run: <outcome>", hands input
// back and is held until the next run starts.
func TestModelRunEndSetsTheOutcome(t *testing.T) {
	m := fresh(t)
	typeLine(m, "change f")
	enter(m)
	feed(m, runEnd(map[string]any{"outcome": "not_converged", "exit": 2, "restored": true}))
	if m.Running {
		t.Fatal("still running after run_end")
	}
	if m.Outcome != "did not converge; worktree restored: true" {
		t.Fatalf("outcome %q", m.Outcome)
	}
	m.Update(tea.WindowSizeMsg{Width: 140, Height: 30})
	if !strings.Contains(m.View(), "last run: did not converge; worktree restored: true") {
		t.Fatalf("view does not show the outcome:\n%s", m.View())
	}
	if last := m.Transcript[len(m.Transcript)-1]; last != "· run finished, exit 2" {
		t.Fatalf("last line %q: the exit code, not a verdict, is reported", last)
	}
	feed(m, turn(1, "unrelated", nil))
	if m.Outcome == "" {
		t.Fatal("the outcome was dropped before the next run")
	}
	typeLine(m, "next")
	enter(m)
	if last := m.Transcript[len(m.Transcript)-1]; last != "> next" {
		t.Fatalf("the second task's echo is not a line of its own: %q", last)
	}
	if m.Outcome != "" {
		t.Fatalf("the outcome %q outlived the start of the next run", m.Outcome)
	}
}

// Every way a run ends hands input back, and each outcome is worded as
// tui_run.rs words it.
func TestModelEveryEndingHandsInputBack(t *testing.T) {
	for _, c := range []struct {
		fields map[string]any
		want   string
	}{
		{map[string]any{"outcome": "converged", "exit": 0}, "converged"},
		{map[string]any{"outcome": "not_converged", "exit": 2, "restored": false}, "did not converge; worktree restored: false"},
		{map[string]any{"outcome": "interrupted", "exit": 130, "restored": true}, "interrupted; worktree restored: true"},
		{map[string]any{"outcome": "error", "exit": 1, "error": "cannot reach the runtime\ncaused by: refused"}, "failed: cannot reach the runtime"},
		{map[string]any{}, "ended; trellis reported no outcome"},
	} {
		m := fresh(t)
		typeLine(m, "t")
		enter(m)
		feed(m, runEnd(c.fields))
		if m.Running || m.Outcome != c.want {
			t.Errorf("%v: running %v, outcome %q, want %q", c.fields, m.Running, m.Outcome, c.want)
		}
	}
}

// A run that fails, before its first turn or after, says why on every line
// of its error, each marked; one whose error renders to nothing still says
// so rather than leaving silence.
func TestModelRunEndErrorMarksEveryLine(t *testing.T) {
	m := fresh(t)
	typeLine(m, "t")
	enter(m)
	feed(m, runEnd(map[string]any{"outcome": "error", "exit": 1, "error": "run failed\n\ncaused by: worktree dirty\n  a.py"}))
	got := m.Transcript[1:]
	want := []string{"! run failed", "! caused by: worktree dirty", "!   a.py"}
	if strings.Join(got, "|") != strings.Join(want, "|") {
		t.Fatalf("got %q, want %q", got, want)
	}
	m = fresh(t)
	typeLine(m, "t")
	enter(m)
	feed(m, runEnd(map[string]any{"outcome": "error", "exit": 1, "error": "  \n"}))
	if last := m.Transcript[len(m.Transcript)-1]; last != "! the run failed and reported no reason" {
		t.Fatalf("an empty error was silence: %q", m.Transcript)
	}
}

// Ctrl-C mid-run sends abort, then quit, in that order, and once; the
// program ends only when the child has exited by itself.
func TestModelCtrlCMidRunSendsAbortThenQuit(t *testing.T) {
	m := fresh(t)
	typeLine(m, "change f")
	enter(m)
	m.TakeOutbox()
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	if got := kinds(m.TakeOutbox()); strings.Join(got, ",") != "abort,QUIT" {
		t.Fatalf("sent %v, want abort then quit", got)
	}
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	if out := m.TakeOutbox(); len(out) != 0 {
		t.Fatalf("a second ^c sent %v", kinds(out))
	}
	if m.Done {
		t.Fatal("done before the child exited; the rollback may still be running")
	}
	feed(m, runEnd(map[string]any{"outcome": "interrupted", "exit": 130, "restored": true}))
	m.Update(ExitedMsg{})
	if !m.Done {
		t.Fatal("not done after the child exited")
	}
}

// Ctrl-C with nothing running sends quit alone, and the program ends when
// the child exits (events.rs: quit ends the loop).
func TestModelCtrlCIdleQuitsAndEndsOnExit(t *testing.T) {
	m := fresh(t)
	m.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	if got := kinds(m.TakeOutbox()); strings.Join(got, ",") != "QUIT" {
		t.Fatalf("sent %v", got)
	}
	m.Update(ExitedMsg{})
	if !m.Done {
		t.Fatal("not done")
	}
}

// Start-up asks for state and names the model: --model's spec as
// set_model {model}, which rpc takes as the user's own choice, or the
// configured one confirmed; a task given on the command line follows it.
func TestModelStartupSetsTheModelBeforeAnyPrompt(t *testing.T) {
	m := New(Options{ModelSpec: "ollama:qwen", Task: "fix it"})
	out := m.TakeOutbox()
	if got := strings.Join(kinds(out), ","); got != "get_state,set_model,prompt" {
		t.Fatalf("sent %s", got)
	}
	if out[1].Cmd["model"] != "ollama:qwen" || out[2].Cmd["task"] != "fix it" {
		t.Fatalf("%+v", out)
	}
	out = New(Options{}).TakeOutbox()
	if out[1].Cmd["configured"] != true || out[1].Cmd["model"] != nil {
		t.Fatalf("no spec given: %+v", out[1].Cmd)
	}
}

// A refused prompt (no model chosen, a run already going) hands input back
// and says why; no run is shown as in flight.
func TestModelRefusedPromptHandsInputBack(t *testing.T) {
	m := fresh(t)
	typeLine(m, "t")
	enter(m)
	id := m.TakeOutbox()[0].Cmd["id"]
	feed(m, jsonLine(map[string]any{"type": "response", "command": "prompt", "id": id, "success": false,
		"error": "choose a model first: set_model"}))
	if m.Running {
		t.Fatal("still running after a refused prompt")
	}
	if last := m.Transcript[len(m.Transcript)-1]; last != "! prompt: choose a model first: set_model" {
		t.Fatalf("last line %q", last)
	}
}

// Served text never carries a control character into the transcript, and
// blank spacing lines do not reach it.
func TestModelServedTextIsPlainAndBlankLinesAreDropped(t *testing.T) {
	m := fresh(t)
	feed(m, "\x1b[2Jcleared\x07")
	feed(m, "   ")
	feed(m, turn(1, "edit \x1b]0;title\x07a.py", nil))
	for _, l := range m.Transcript {
		if strings.ContainsAny(l, "\x1b\x07") {
			t.Fatalf("a control character reached the transcript: %q", l)
		}
	}
	if len(m.Transcript) != 2 || m.Transcript[0] != "[2Jcleared" {
		t.Fatalf("transcript %q", m.Transcript)
	}
}

// A child that exits without being asked is said to have, and the
// worktree's state is not claimed.
func TestModelChildExitMidRunIsReported(t *testing.T) {
	m := fresh(t)
	typeLine(m, "t")
	enter(m)
	m.Update(ExitedMsg{Err: errors.New("exit status 101")})
	all := strings.Join(m.Transcript, "\n")
	if !m.Done || m.Running || !strings.Contains(all, "exited unexpectedly: exit status 101") ||
		!strings.Contains(all, "git status") {
		t.Fatalf("done %v running %v transcript:\n%s", m.Done, m.Running, all)
	}
}

// fakeClient records what App writes.
type fakeClient struct {
	sent  []string
	lines chan rpc.Line
}

func (f *fakeClient) Send(c map[string]any) error {
	f.sent = append(f.sent, c["type"].(string))
	return nil
}
func (f *fakeClient) Quit() error             { f.sent = append(f.sent, "QUIT"); return nil }
func (f *fakeClient) Lines() <-chan rpc.Line  { return f.lines }
func (f *fakeClient) Stderr() ([]string, int) { return []string{"warming up"}, 0 }
func (f *fakeClient) Wait() error             { return nil }

// App writes the outbox to the child in the order the model queued it, and
// gives stderr to the model as text, once.
func TestModelAppWritesTheOutboxInOrder(t *testing.T) {
	f := &fakeClient{lines: make(chan rpc.Line)}
	a := NewApp(New(Options{Task: "t"}), f)
	a.Init()
	a.Update(tea.KeyMsg{Type: tea.KeyCtrlC})
	if got := strings.Join(f.sent, ","); got != "get_state,set_model,prompt,abort,QUIT" {
		t.Fatalf("sent %s", got)
	}
	a.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	if len(a.M.Stderr) != 1 || a.M.Stderr[0] != "warming up" {
		t.Fatalf("stderr %q", a.M.Stderr)
	}
}

// A model spec trellis refuses in its set_model answer is shown, so a run
// refused for it later is not a mystery; the spec is trellis's to parse.
func TestModelFailedSetModelIsShown(t *testing.T) {
	m := New(Options{ModelSpec: "nonsense"})
	id := m.TakeOutbox()[1].Cmd["id"]
	feed(m, jsonLine(map[string]any{"type": "response", "command": "set_model", "id": id, "success": false,
		"error": "unknown model spec `nonsense`\nexpected <backend>:<model>"}))
	if len(m.Transcript) != 1 || m.Transcript[0] != "! set_model: unknown model spec `nonsense`" {
		t.Fatalf("transcript %q", m.Transcript)
	}
}

func windowSize(w, h int) tea.WindowSizeMsg { return tea.WindowSizeMsg{Width: w, Height: h} }

const (
	keyUp   = tea.KeyUp
	keyDown = tea.KeyDown
)

func keyType(k tea.KeyType) tea.KeyMsg { return tea.KeyMsg{Type: k} }
