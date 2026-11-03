package model

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"unicode/utf8"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/rpc"
	"github.com/maydaythecoder/trellis/tui/internal/theme"
	"github.com/maydaythecoder/trellis/tui/internal/theme/themetest"
	"github.com/maydaythecoder/trellis/tui/internal/view"
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
	submit(m, "fix the bug")
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "prompt" || out[0].Cmd["task"] != "fix the bug" {
		t.Fatalf("outbox %+v", out)
	}
	if out[0].Cmd["id"] == nil {
		t.Fatal("the prompt has no id to match its run_end by")
	}
	if !m.Running || m.Input != "" || texts(m)[len(m.Transcript)-1] != "you · task  fix the bug" {
		t.Fatalf("running %v, input %q, transcript %q", m.Running, m.Input, texts(m))
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

// Each turn frame adds its rows on their own, never on the end of the task
// echo: its divider, its call, and a check row with the verdict trellis
// reported and nothing computed.
func TestModelTurnsFillTheTranscript(t *testing.T) {
	m := fresh(t)
	submit(m, "change f")
	feed(m, turn(1, "read_file a.py", nil))
	feed(m, turn(2, "run_check tests", map[string]any{"verdict": "Some(true)"}))
	feed(m, turn(3, "run_check types", map[string]any{"verdict": "Some(false)"}))
	feed(m, turn(4, "edit a.py", map[string]any{"verdict": "None"}))
	want := []string{
		"you · task  change f",
		"─ turn 1 · model — ms · — tok out · — B prompt",
		"· read_file a.py",
		"─ turn 2 · model — ms · — tok out · — B prompt",
		"· run_check tests",
		"✓ check passed · trellis-checks verdict",
		"─ turn 3 · model — ms · — tok out · — B prompt",
		"· run_check types",
		"✗ check failed · trellis-checks verdict",
		"─ turn 4 · model — ms · — tok out · — B prompt",
		"· edit a.py",
	}
	if strings.Join(texts(m), "\n") != strings.Join(want, "\n") {
		t.Fatalf("transcript:\n%s\nwant:\n%s", strings.Join(texts(m), "\n"), strings.Join(want, "\n"))
	}
}

// The transcript keeps the newest TranscriptCap lines, as the Rust TUI
// does: a long run would otherwise grow it without bound. Each turn here
// writes two rows, its divider and its call (#947), so the newest
// TranscriptCap/2 turns are the ones kept, the oldest of them whole.
func TestModelTranscriptCapHolds(t *testing.T) {
	m := fresh(t)
	for i := 1; i <= TranscriptCap+500; i++ {
		feed(m, turn(i, fmt.Sprintf("step %d", i), nil))
	}
	if len(m.Transcript) != TranscriptCap {
		t.Fatalf("%d lines kept, want %d", len(m.Transcript), TranscriptCap)
	}
	first := uint64(TranscriptCap+500) - TranscriptCap/2 + 1
	if texts(m)[0] != view.TurnDivider(first, nil, nil, nil) || texts(m)[1] != fmt.Sprintf("· step %d", first) ||
		texts(m)[TranscriptCap-1] != fmt.Sprintf("· step %d", TranscriptCap+500) {
		t.Fatalf("kept %q, %q .. %q", texts(m)[0], texts(m)[1], texts(m)[TranscriptCap-1])
	}
}

// run_end sets the outcome, shown as "last run: <outcome>", hands input
// back and is held until the next run starts.
func TestModelRunEndSetsTheOutcome(t *testing.T) {
	m := fresh(t)
	submit(m, "change f")
	feed(m, runEnd(map[string]any{"outcome": "not_converged", "exit": 2, "restored": true}))
	if m.Running {
		t.Fatal("still running after run_end")
	}
	if m.Outcome != "not_converged · exit 2; worktree restored: true" {
		t.Fatalf("outcome %q", m.Outcome)
	}
	m.Update(tea.WindowSizeMsg{Width: 140, Height: 30})
	if !strings.Contains(m.View(), "last run: not_converged · exit 2; worktree restored: true") {
		t.Fatalf("view does not show the outcome:\n%s", m.View())
	}
	if last := texts(m)[len(m.Transcript)-1]; last != "· run finished, exit 2" {
		t.Fatalf("last line %q: the exit code, not a verdict, is reported", last)
	}
	feed(m, turn(1, "unrelated", nil))
	if m.Outcome == "" {
		t.Fatal("the outcome was dropped before the next run")
	}
	submit(m, "next")
	if last := texts(m)[len(m.Transcript)-1]; last != "you · task  next" {
		t.Fatalf("the second task's echo is not a line of its own: %q", last)
	}
	if m.Outcome != "" {
		t.Fatalf("the outcome %q outlived the start of the next run", m.Outcome)
	}
}

// Every way a run ends hands input back, and each outcome is run_end's own
// word with its exit code (DESIGN.md section 2 rule 5, #803).
func TestModelEveryEndingHandsInputBack(t *testing.T) {
	for _, c := range []struct {
		fields map[string]any
		want   string
	}{
		{map[string]any{"outcome": "converged", "exit": 0}, "converged · exit 0"},
		{map[string]any{"outcome": "not_converged", "exit": 2, "restored": false}, "not_converged · exit 2; worktree restored: false"},
		{map[string]any{"outcome": "interrupted", "exit": 130, "restored": true}, "interrupted · exit 130; worktree restored: true"},
		{map[string]any{"outcome": "error", "exit": 1, "error": "cannot reach the runtime\ncaused by: refused"}, "error · exit 1: cannot reach the runtime"},
		{map[string]any{}, "ended; trellis reported no outcome"},
	} {
		m := fresh(t)
		submit(m, "t")
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
	submit(m, "t")
	feed(m, runEnd(map[string]any{"outcome": "error", "exit": 1, "error": "run failed\n\ncaused by: worktree dirty\n  a.py"}))
	got := texts(m)[1:]
	want := []string{"! run failed", "! caused by: worktree dirty", "!   a.py"}
	if strings.Join(got, "|") != strings.Join(want, "|") {
		t.Fatalf("got %q, want %q", got, want)
	}
	m = fresh(t)
	submit(m, "t")
	feed(m, runEnd(map[string]any{"outcome": "error", "exit": 1, "error": "  \n"}))
	if last := texts(m)[len(m.Transcript)-1]; last != "! the run failed and reported no reason" {
		t.Fatalf("an empty error was silence: %q", texts(m))
	}
}

// Ctrl-C mid-run sends abort, then quit, in that order, and once; the
// program ends only when the child has exited by itself.
func TestModelCtrlCMidRunSendsAbortThenQuit(t *testing.T) {
	m := fresh(t)
	submit(m, "change f")
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
	// The task is asked about first, as a typed line is (route.go).
	if got := strings.Join(kinds(out), ","); got != "get_state,set_model,classify" {
		t.Fatalf("sent %s", got)
	}
	if out[1].Cmd["model"] != "ollama:qwen" || out[2].Cmd["text"] != "fix it" {
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
	submit(m, "t")
	id := m.TakeOutbox()[0].Cmd["id"]
	feed(m, jsonLine(map[string]any{"type": "response", "command": "prompt", "id": id, "success": false,
		"error": "choose a model first: set_model"}))
	if m.Running {
		t.Fatal("still running after a refused prompt")
	}
	if last := texts(m)[len(m.Transcript)-1]; last != "! prompt: choose a model first: set_model" {
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
	for _, l := range texts(m) {
		if strings.ContainsAny(l, "\x1b\x07") {
			t.Fatalf("a control character reached the transcript: %q", l)
		}
	}
	if len(m.Transcript) != 3 || texts(m)[0] != "[2Jcleared" {
		t.Fatalf("transcript %q", texts(m))
	}
}

// A child that exits without being asked is said to have, and the
// worktree's state is not claimed.
func TestModelChildExitMidRunIsReported(t *testing.T) {
	m := fresh(t)
	submit(m, "t")
	m.Update(ExitedMsg{Err: errors.New("exit status 101")})
	all := strings.Join(texts(m), "\n")
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
	// abort also drops the pending classify, unanswered.
	if got := strings.Join(f.sent, ","); got != "get_state,set_model,classify,abort,QUIT" {
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
	if len(m.Transcript) != 1 || texts(m)[0] != "! set_model: unknown model spec `nonsense`" {
		t.Fatalf("transcript %q", texts(m))
	}
}

func windowSize(w, h int) tea.WindowSizeMsg { return tea.WindowSizeMsg{Width: w, Height: h} }

const (
	keyUp   = tea.KeyUp
	keyDown = tea.KeyDown
)

func keyType(k tea.KeyType) tea.KeyMsg { return tea.KeyMsg{Type: k} }

// submit types a line, sends it, and answers trellis's classify as a task,
// so the prompt follows (route.go).
func submit(m *Model, text string) {
	typeLine(m, text)
	enter(m)
	var keep []Out
	for _, o := range m.TakeOutbox() {
		if o.Cmd != nil && o.Cmd["type"] == "classify" {
			defer respond(m, o.Cmd["id"], "classify", map[string]any{"intent": "task"})
			continue
		}
		keep = append(keep, o)
	}
	m.outbox = append(m.outbox, keep...)
}

// #639: one paste made one submission per line in the ratatui TUI. A
// paste is one input: its breaks are spaces, one per break whether the
// terminal sent CR, LF or both, and nothing is sent until Enter.
func TestModelPasteIsOneInputAndSubmitsNothing(t *testing.T) {
	m := fresh(t)
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("a\nb"), Paste: true})
	if m.Input != "a b" {
		t.Fatalf("input %q", m.Input)
	}
	if out := m.TakeOutbox(); len(out) != 0 || m.Running {
		t.Fatalf("a paste sent %v", out)
	}
	enter(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "classify" || out[0].Cmd["text"] != "a b" {
		t.Fatalf("enter sent %v", out)
	}

	m = fresh(t)
	typeLine(m, "x ")
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("a\r\nb\rc\n\x1b[2J"), Paste: true})
	if m.Input != "x a b c [2J" {
		t.Fatalf("CR, LF and CRLF are one space each, and no control character is kept: %q", m.Input)
	}
}

// pasteProbe records the keys a real Bubble Tea program delivers.
type pasteProbe struct{ keys []tea.KeyMsg }

func (p *pasteProbe) Init() tea.Cmd { return nil }
func (p *pasteProbe) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	if k, ok := msg.(tea.KeyMsg); ok {
		p.keys = append(p.keys, k)
		return p, tea.Quit
	}
	return p, nil
}
func (p *pasteProbe) View() string { return "" }

// The terminal half (events.rs's paste_event_becomes_one_paste_message):
// a bracketed paste reaches Update as one key carrying the whole text,
// breaks included, not as keystrokes with an Enter per line. trellis-tui
// starts its program without WithoutBracketedPaste, so this is the path.
func TestModelPasteArrivesAsOneKey(t *testing.T) {
	probe := &pasteProbe{}
	in := strings.NewReader("\x1b[200~a\nb\x1b[201~")
	p := tea.NewProgram(probe, tea.WithInput(in), tea.WithOutput(io.Discard), tea.WithoutSignals())
	if _, err := p.Run(); err != nil {
		t.Fatalf("program: %v", err)
	}
	if len(probe.keys) != 1 {
		t.Fatalf("got %d keys, want one: %v", len(probe.keys), probe.keys)
	}
	k := probe.keys[0]
	if !k.Paste || string(k.Runes) != "a\nb" {
		t.Fatalf("got %+v", k)
	}
}

// The theme the program built from the terminal reaches the screen and the
// approval modal: both are painted with it, and with no theme nothing is
// painted, as on a terminal with no colour (#841).
func TestModelPaintsWithTheThemeItIsGiven(t *testing.T) {
	th := theme.New(themetest.Renderer(t, "TrueColor", true))
	m := New(Options{Theme: th})
	m.TakeOutbox()
	m.Update(windowSize(100, 30))
	if v := m.View(); !strings.Contains(v, th.Line.Render("┌")) || !strings.Contains(v, th.InkSecondary.Render(" transcript ")) {
		t.Fatalf("the screen is not painted with the theme:\n%q", v)
	}
	feed(m, approval("a1", "edit a.py", "+ one line"))
	if v := m.View(); !strings.Contains(v, th.Line.Render("┌")) || !strings.Contains(v, th.Ink.Render("+ one line")) {
		t.Fatalf("the modal is not painted with the theme:\n%q", v)
	}
	plain := fresh(t)
	plain.Update(windowSize(100, 30))
	if v := plain.View(); strings.Contains(v, "\x1b") {
		t.Fatalf("with no theme the screen carries an escape sequence:\n%q", v)
	}
}

// #854: the model's closing words are asked for only after a converged
// run, since trellis keeps the words of the last run that ended on done and
// after any other outcome they could be an earlier run's. The answer is
// read only while that run's question is open, so a reply that comes after
// a new run has started is not shown as the new run's; it is cut to
// ClosingCap, with what was cut said; and a refusal or no words is said
// with why rather than drawn as words.
func TestSummaryClosingWordsAskedOnlyAfterAConvergedRun(t *testing.T) {
	closingAsk := func(out []Out) any {
		for _, o := range out {
			if o.Cmd != nil && o.Cmd["type"] == "get_last_assistant_text" {
				return o.Cmd["id"]
			}
		}
		return nil
	}
	m := running(t)
	feed(m, runEnd(map[string]any{"outcome": "not_converged", "exit": 1, "restored": true}))
	if id := closingAsk(m.TakeOutbox()); id != nil {
		t.Fatalf("a run that did not converge asked for closing words")
	}
	if c := m.closingState(); c.Words != nil || !strings.Contains(c.Why, "did not converge") {
		t.Fatalf("%+v", c)
	}

	submit(m, "again")
	m.TakeOutbox()
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	id := closingAsk(m.TakeOutbox())
	if id == nil {
		t.Fatalf("a converged run did not ask for its closing words")
	}
	long := strings.Repeat("é", ClosingCap) + "\x1b[2J"
	respond(m, id, "get_last_assistant_text", map[string]any{"text": long})
	c := m.closingState()
	if c.Words == nil || len(*c.Words) > ClosingCap || !utf8.ValidString(*c.Words) || c.Total != len(long) || c.Cut != len(long)-len(*c.Words) {
		t.Fatalf("cut %d of %d, kept %d bytes", c.Cut, c.Total, len(*c.Words))
	}

	// A new run forgets the words, and a late answer to the last run's
	// question is not read as its own.
	submit(m, "third")
	m.TakeOutbox()
	respond(m, id, "get_last_assistant_text", map[string]any{"text": "the last run's words"})
	if c := m.closingState(); c.Words != nil || !strings.Contains(c.Why, "once the run has converged") {
		t.Fatalf("the new run shows %+v", c)
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	id = closingAsk(m.TakeOutbox())
	respond(m, id, "get_last_assistant_text", map[string]any{"text": nil})
	if c := m.closingState(); c.Words != nil || !strings.Contains(c.Why, "no run of this trellis rpc has ended on done") {
		t.Fatalf("no words: %+v", c)
	}

	submit(m, "fourth")
	m.TakeOutbox()
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	id = closingAsk(m.TakeOutbox())
	feed(m, jsonLine(map[string]any{"type": "response", "command": "get_last_assistant_text", "id": id, "success": false, "error": "busy"}))
	if c := m.closingState(); c.Words != nil || c.Why != "get_last_assistant_text refused: busy" {
		t.Fatalf("refused: %+v", c)
	}
}

// clientFixtures is crates/trellis-cli/tests/client_fixtures.json, the
// cases the desktop's client_fixtures_ tests walk too (#803).
type clientFixtures struct {
	OutcomeWords struct {
		Cases []struct {
			Name   string         `json:"name"`
			RunEnd map[string]any `json:"run_end"`
			Word   string         `json:"word"`
			Exit   int            `json:"exit"`
		} `json:"cases"`
	} `json:"outcome_words"`
	ToolSentences struct {
		Cases []struct {
			Name     string         `json:"name"`
			Turn     map[string]any `json:"turn"`
			Sentence string         `json:"sentence"`
			Check    *string        `json:"check"`
		} `json:"cases"`
	} `json:"tool_sentences"`
	HostileText struct {
		MustNotContain []string `json:"must_not_contain"`
		Cases          []struct {
			Name   string `json:"name"`
			Text   string `json:"text"`
			Repeat int    `json:"repeat"`
			TUI    string `json:"tui"`
		} `json:"cases"`
	} `json:"hostile_text"`
}

func readClientFixtures(t *testing.T) clientFixtures {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join("..", "..", "..", "crates", "trellis-cli", "tests", "client_fixtures.json"))
	if err != nil {
		t.Fatal(err)
	}
	var f clientFixtures
	if err := json.Unmarshal(raw, &f); err != nil {
		t.Fatalf("client_fixtures.json: %v", err)
	}
	return f
}

// The risk (#803): the two faces word one run's end two ways ("did not
// converge" here, "not converged" on the desktop, "failed" for an error),
// so one run reads as two. Every run_end trellis emits is shown as its own
// word and its exit code, the wording DESIGN.md's rule 5 gives.
func TestClientFixturesOutcomeWords(t *testing.T) {
	f := readClientFixtures(t)
	if len(f.OutcomeWords.Cases) == 0 {
		t.Fatal("no outcome cases")
	}
	for _, c := range f.OutcomeWords.Cases {
		m := running(t)
		feed(m, runEnd(c.RunEnd))
		want := fmt.Sprintf("%s · exit %d", c.Word, c.Exit)
		if !strings.HasPrefix(m.Outcome, want) {
			t.Errorf("%s: outcome %q, want it to start %q", c.Name, m.Outcome, want)
		}
		if s := stripOf(t, m); !strings.Contains(s, "ENDED · "+want) {
			t.Errorf("%s: the Now strip does not say %q:\n%s", c.Name, "ENDED · "+want, s)
		}
	}
}

// The risk (#803): a tool call read one way here and another on the
// desktop, or worded from the call's own text, which the model writes.
// Each turn is the one sentence both faces show, from the engine's tool
// and target, and a check row's words are the kind and its verdict's word,
// on a row of their own (#947), as the desktop's check entry is.
func TestClientFixturesToolSentences(t *testing.T) {
	f := readClientFixtures(t)
	if len(f.ToolSentences.Cases) == 0 {
		t.Fatal("no tool cases")
	}
	for _, c := range f.ToolSentences.Cases {
		m := running(t)
		fields := map[string]any{"message": "turn"}
		for k, v := range c.Turn {
			fields[k] = v
		}
		feed(m, jsonLine(map[string]any{"level": "INFO", "fields": fields}))
		var row string
		var checks []view.Row
		for _, r := range m.Transcript {
			if strings.HasPrefix(r.Text, "· ") {
				row = r.Text
			}
			if r.Role == view.RolePassed || r.Role == view.RoleFailed {
				checks = append(checks, r)
			}
		}
		if want := "· " + c.Sentence; row != want {
			t.Errorf("%s: row %q, want %q", c.Name, row, want)
		}
		if c.Check == nil {
			if len(checks) != 0 {
				t.Errorf("%s: a turn with no verdict wrote check rows %+v", c.Name, checks)
			}
			continue
		}
		mark, role := "✓", view.RolePassed
		if strings.HasSuffix(*c.Check, " failed") {
			mark, role = "✗", view.RoleFailed
		}
		want := mark + " " + *c.Check + " · trellis-checks verdict"
		if len(checks) != 1 || checks[0].Text != want || checks[0].Role != role {
			t.Errorf("%s: check rows %+v, want one %q in role %d", c.Name, checks, want, role)
		}
	}
}

// The risk (#803): text from the model or the repository that starts an
// escape sequence, reorders what is drawn with a bidirectional override
// (a filename or a line that reads as something else), carries a NUL, or
// is one line of megabytes. Plain drops each listed character, and a turn
// made of the text writes no row that holds one or runs past LineCap.
func TestClientFixturesHostileText(t *testing.T) {
	f := readClientFixtures(t)
	if len(f.HostileText.Cases) == 0 || len(f.HostileText.MustNotContain) == 0 {
		t.Fatal("no hostile cases")
	}
	for _, c := range f.HostileText.Cases {
		text := c.Text
		if c.Repeat > 0 {
			text = strings.Repeat(c.Text, c.Repeat)
		} else if got := view.Plain(text); got != c.TUI {
			t.Errorf("%s: Plain gave %q, want %q", c.Name, got, c.TUI)
		}
		m := running(t)
		feed(m, turn(1, text, map[string]any{"tool": "bash", "target": text, "wrote": false, "verdict": "None", "check_kind": "", "observation": text}))
		feed(m, runEnd(map[string]any{"outcome": "error", "exit": 1, "error": text}))
		rows := texts(m)
		longest := LineCap + len(fmt.Sprintf("… [%d more bytes cut]", len(text)))
		for _, l := range rows {
			for _, bad := range f.HostileText.MustNotContain {
				if strings.Contains(l, bad) {
					t.Errorf("%s: a row holds %q: %q", c.Name, bad, l)
				}
			}
			if len(l) > longest {
				t.Errorf("%s: a row of %d bytes, past LineCap", c.Name, len(l))
			}
		}
		m.Update(windowSize(120, 40))
		screen := m.View()
		for _, bad := range f.HostileText.MustNotContain {
			if strings.Contains(m.Outcome, bad) || strings.Contains(screen, bad) {
				t.Errorf("%s: the outcome or the screen holds %q", c.Name, bad)
			}
		}
		if c.Repeat > 0 && !strings.Contains(strings.Join(rows, "\n"), "more bytes cut]") {
			t.Errorf("%s: a long line was not cut, or not said to be", c.Name)
		}
	}
}
