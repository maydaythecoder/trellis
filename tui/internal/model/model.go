// Package model is the Go TUI's state and the one function allowed to
// change it, the counterpart of the ratatui TUI's AppState::update
// (crates/tui-demo/src/tui/app.rs).
//
// Model.Update is pure: no IO, no channel, no process. What it decides to
// send to trellis is recorded in an outbox, which the program drains and
// writes to the rpc child (App in app.go). That is what lets every test in
// this package run without a terminal and without trellis.
//
// Everything a frame carries is untrusted (model or server text), so every
// string that reaches the transcript has its control characters removed
// first (Plain).
package model

import (
	"fmt"
	"strings"
	"time"
	"unicode"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// TranscriptCap is how many transcript lines are kept, as the ratatui
// TUI's TRANSCRIPT_CAP: a long run would otherwise grow it without bound.
const TranscriptCap = 2000

// StderrCap is how many of trellis's stderr lines the model keeps for the
// capped messages pane (ADR 0052 section 4). Human text only.
const StderrCap = 200

// Out is one thing to send to the rpc child: a command, or the quit that
// closes its stdin.
type Out struct {
	Cmd  map[string]any
	Quit bool
}

// Options are what the program was started with.
type Options struct {
	// Task, when set, is sent as the first prompt.
	Task string
	// ModelSpec is --model's value: sent as set_model {model}, which rpc
	// takes as the user's own choice (ADR 0024 section 3). Empty confirms
	// the configured model with set_model {configured: true}.
	ModelSpec string
}

// Model is the TUI's whole state.
type Model struct {
	// Transcript holds whole lines, oldest first, capped at TranscriptCap.
	Transcript []string
	// Input is the line being typed.
	Input string
	// Running is true from a prompt being sent until its run_end, or a
	// failed prompt response. One run at a time.
	Running bool
	// Outcome is how the last run ended, until the next one starts.
	Outcome string
	// Stderr is trellis's stderr, newest last, capped at StderrCap.
	Stderr []string
	// Modal is the approval question on screen, if any. While it is up it
	// takes the keys: y and n answer it, every other key but ^c is ignored.
	Modal *Approval

	// approvals waiting behind the one on screen, in the order asked.
	approvals []Approval
	// steers sent to the run in flight, in order (steer.go).
	steers []steer
	// deliveries counts this run's steer delivered events.
	deliveries int

	// Quitting is set once quit has been sent: the child is finishing (a
	// run in flight rolls back) and the program ends when it exits.
	Quitting bool
	// Done is set when the child has exited; the program then ends.
	Done bool

	Width, Height int

	// The run panes (panes.go): what the agent did, the edits it made, the
	// checks trellis ran, the last blast radius and the budget.
	Actions []view.Action
	Edits   []view.Edit
	Checks  []view.Check
	Radius  *view.Radius
	Budget  view.Budget
	// ScrollBack is how many rows above the transcript's end the view is
	// scrolled; 0 follows new output.
	ScrollBack int
	// ModelName and Tier are what the status bar says (status.go).
	ModelName, Tier string
	// Offered are the @ completions on offer and, when trellis sent more
	// than OfferedCap, a last line saying how many more (complete.go).
	Offered []string

	// restored is the last run_end's restored, for the bar's worktree
	// state (#636); nil until a run has ended and said.
	restored *bool

	// now is the clock the budget's elapsed time is read from; tests set
	// it. runStart is when the run in flight started.
	now      func() time.Time
	runStart time.Time

	// purposes maps a sent command's id to what its answer is read for
	// (status.go).
	purposes map[string]string
	// offeredFor is the @fragment Offered was listed for, and pathsID the
	// list_paths request in flight (complete.go).
	offeredFor, pathsID string
	// setModelID is the set_model whose success means the tier may now be
	// known.
	setModelID string
	// classifying is set while trellis is asked whether a line is a
	// question or a task, and classifyText is that line (route.go).
	classifying  bool
	classifyText string

	outbox []Out
	nextID int
	// The id of the prompt in flight, to match its response and run_end.
	promptID string
}

// New is a model that, once its outbox is drained, has asked trellis for
// its state, chosen the model and, with a task given, sent it.
func New(o Options) *Model {
	m := &Model{now: time.Now}
	m.refreshState()
	if o.ModelSpec != "" {
		m.setModelID = m.send("set_model", map[string]any{"model": o.ModelSpec})
	} else {
		m.setModelID = m.send("set_model", map[string]any{"configured": true})
	}
	if t := strings.TrimSpace(o.Task); t != "" {
		m.route(t)
	}
	return m
}

// TakeOutbox returns what is waiting to be sent, in order, and empties it.
func (m *Model) TakeOutbox() []Out {
	out := m.outbox
	m.outbox = nil
	return out
}

// send queues one command with an id of its own.
func (m *Model) send(kind string, fields map[string]any) string {
	m.nextID++
	id := fmt.Sprintf("tui-%d", m.nextID)
	cmd := map[string]any{"id": id, "type": kind}
	for k, v := range fields {
		cmd[k] = v
	}
	m.outbox = append(m.outbox, Out{Cmd: cmd})
	return id
}

// quit queues quit, once. trellis then aborts any run, waits for its
// rollback and exits by itself; nothing here kills it.
func (m *Model) quit() {
	if m.Quitting {
		return
	}
	m.Quitting = true
	m.outbox = append(m.outbox, Out{Quit: true})
}

// Messages the program feeds Update besides keys and window sizes.
type (
	// FrameMsg is one typed line from the child.
	FrameMsg struct{ Frame frames.Frame }
	// ExitedMsg says the child has exited, with how, when it was not 0.
	ExitedMsg struct{ Err error }
	// SendFailedMsg says a write to the child failed.
	SendFailedMsg struct{ Err error }
	// TickMsg is the program's clock, once a second (App), so the budget's
	// elapsed time moves between turns.
	TickMsg struct{}
)

// Update applies one message. It does no IO.
func (m *Model) Update(msg tea.Msg) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		m.key(msg)
	case tea.WindowSizeMsg:
		m.Width, m.Height = msg.Width, msg.Height
	case FrameMsg:
		m.frame(msg.Frame)
	case ExitedMsg:
		m.exited(msg.Err)
	case SendFailedMsg:
		m.pushLine("! could not write to trellis rpc: " + Plain(msg.Err.Error()))
	case TickMsg:
		m.tick()
	}
}

func (m *Model) key(k tea.KeyMsg) {
	if k.Type == tea.KeyCtrlC {
		m.ctrlC()
		return
	}
	// The modal is answered first: while it is up, the keys underneath it
	// are not the ones on screen.
	if m.Modal != nil {
		m.modalKey(k)
		return
	}
	switch k.Type {
	case tea.KeyEsc:
		// Esc clears the line; on an empty line it leaves as ^c does,
		// stopping a run in flight first (#634): leaving mid-run used to
		// be ^c's alone, and the ratatui TUI now stops on both.
		if m.Input != "" {
			m.Input = ""
		} else {
			m.ctrlC()
		}
	case tea.KeyEnter:
		m.submit()
	case tea.KeyTab:
		m.tab()
	case tea.KeyBackspace:
		r := []rune(m.Input)
		if len(r) > 0 {
			m.Input = string(r[:len(r)-1])
		}
		m.inputChanged()
	case tea.KeySpace:
		m.Input += " "
		m.inputChanged()
	case tea.KeyRunes:
		if k.Paste {
			m.Input += pasted(k.Runes)
		} else {
			m.Input += string(k.Runes)
		}
		m.inputChanged()
	case tea.KeyUp:
		m.ScrollBack++
	case tea.KeyDown:
		// Saturating: Down at the end stays at the end, following.
		if m.ScrollBack > 0 {
			m.ScrollBack--
		}
	case tea.KeyPgUp:
		// Held at the top, so PgDn after it moves at once rather than
		// first spending presses on rows above the first (#660).
		m.ScrollBack = min(m.ScrollBack+view.TranscriptPage, m.transcriptTop())
	case tea.KeyPgDown:
		m.ScrollBack = max(m.ScrollBack-view.TranscriptPage, 0)
	case tea.KeyHome:
		m.ScrollBack = m.transcriptTop()
	case tea.KeyEnd:
		m.ScrollBack = 0
	}
}

// transcriptTop is the scroll back that shows the transcript's first row
// at the current width.
func (m *Model) transcriptTop() int {
	return view.TranscriptRows(m.viewState(), m.Width)
}

// ctrlC aborts a run in flight first, so it rolls back through trellis,
// then quits; the program ends when the child exits. An approval on screen
// is closed unanswered: the run's abort answers its gate no, and nothing
// here turns leaving into a yes. A question still on its way is not shown
// either (approvalRequested): nobody is waiting for its answer.
func (m *Model) ctrlC() {
	if m.Quitting {
		return
	}
	m.Modal = nil
	m.approvals = nil
	if m.Running {
		m.send("abort", nil)
		m.pushLine("· stopping: the run rolls back before trellis exits")
	} else if m.classifying {
		// abort also drops a pending classify, unanswered.
		m.send("abort", nil)
	}
	m.quit()
}

// pasted is a bracketed paste as input text (#639). Bubble Tea delivers a
// paste as one key carrying every rune, its line breaks among them; each
// break, CR, LF or CRLF, becomes one space, so a paste never submits
// anything (a break is not an Enter) and a multi-line task is one line
// until Enter sends it. Any other control character is dropped, as Plain
// drops it from served text.
func pasted(r []rune) string {
	s := strings.ReplaceAll(string(r), "\r\n", " ")
	s = strings.NewReplacer("\r", " ", "\n", " ").Replace(s)
	return Plain(s)
}

// submit hands the typed line to trellis: a task when no run is in
// flight, one at a time, and a steer for the run when one is (steer.go).
func (m *Model) submit() {
	task := strings.TrimSpace(m.Input)
	if task == "" || m.Quitting {
		return
	}
	if m.Running {
		m.steer(task)
		return
	}
	if m.classifying {
		m.pushLine("· still asking trellis whether the last line was a question or a task; wait for its answer")
		return
	}
	m.Input = ""
	m.Offered = nil
	if m.local(task) {
		return
	}
	m.route(task)
}

// startRun sends a line trellis classified as a task as prompt, its echo
// already in the transcript.
func (m *Model) startRun(task string) {
	m.newRun()
	m.clearPanes()
	m.Running = true
	m.Outcome = ""
	// A new run is followed from its start, wherever the view was.
	m.ScrollBack = 0
	m.promptID = m.send("prompt", map[string]any{"task": task})
}

func (m *Model) frame(f frames.Frame) {
	switch f.Kind {
	case frames.Turn:
		m.turn(f.TurnFields)
	case frames.RunStart:
		if !m.Running {
			// A run trellis started on its own (a follow-up): steers are
			// counted, and the panes filled, from here.
			m.newRun()
			m.clearPanes()
		}
		m.runStart = m.now()
		m.Running = true
		m.Outcome = ""
	case frames.ApprovalRequest:
		m.approvalRequested(*f.Approval)
	case frames.SteerDelivered:
		m.steerDeliveredEvent(f.AfterStep)
	case frames.RunBudget:
		m.runBudget(f.MaxIterations)
	case frames.RunEnd:
		m.runEnd(f.RunEndFields)
	case frames.Response:
		m.response(f.ResponseFields)
	case frames.Warning, frames.Unparsed:
		m.pushText(f.Text)
	case frames.Human:
		m.Stderr = append(m.Stderr, Plain(f.Text))
		if over := len(m.Stderr) - StderrCap; over > 0 {
			m.Stderr = append([]string(nil), m.Stderr[over:]...)
		}
	}
}

// turn adds the turn's line: what was called and, when a check ran, what
// trellis said it returned. The verdict is shown, never computed (ADR 0004).
func (m *Model) turn(t *frames.TurnFields) {
	line := "· " + Plain(t.Call)
	if t.Verdict != nil {
		switch *t.Verdict {
		case "Some(true)":
			line += "  ✓ check passed"
		case "Some(false)":
			line += "  ✗ check FAILED"
		}
	}
	m.pushLine(line)
	// The observation's first lines under it, as tui_bridge.rs shows them.
	if t.Observation != nil {
		n := 0
		for _, l := range strings.Split(*t.Observation, "\n") {
			if n == 6 {
				break
			}
			if strings.TrimSpace(l) == "" && n == 0 {
				continue
			}
			m.pushLine("    " + Plain(l))
			n++
		}
	}
	m.panesTurn(t)
}

// runEnd hands input back, whatever the ending, and says how it ended from
// the frame's outcome and exit, never from model text.
func (m *Model) runEnd(e *frames.RunEndFields) {
	m.Running = false
	m.promptID = ""
	m.refreshState()
	m.steersNotDelivered()
	// A question its run no longer waits on cannot be answered.
	m.Modal = nil
	m.approvals = nil
	m.Outcome = OutcomeLine(e)
	m.restored = e.Restored
	if e.Outcome != nil && *e.Outcome == "error" {
		m.errorLines(e.Error)
		return
	}
	if e.Exit != nil {
		m.pushLine(fmt.Sprintf("· run finished, exit %d", *e.Exit))
	} else {
		m.pushLine("· run finished; trellis reported no exit code")
	}
}

// errorLines writes an error as lines each marked "! ", and an error that
// renders to nothing as a line that says so rather than as silence.
func (m *Model) errorLines(e *string) {
	said := false
	if e != nil {
		for _, l := range strings.Split(*e, "\n") {
			if strings.TrimSpace(l) == "" {
				continue
			}
			m.pushLine("! " + Plain(l))
			said = true
		}
	}
	if !said {
		m.pushLine("! the run failed and reported no reason")
	}
}

// OutcomeLine is how a run ended, in one line for the transcript's border,
// worded as tui_run.rs's outcome_line words it.
func OutcomeLine(e *frames.RunEndFields) string {
	restored := func() string {
		if e.Restored == nil {
			return "not reported"
		}
		return fmt.Sprint(*e.Restored)
	}
	if e.Outcome == nil {
		return "ended; trellis reported no outcome"
	}
	switch *e.Outcome {
	case "converged":
		return "converged"
	case "not_converged":
		return "did not converge; worktree restored: " + restored()
	case "interrupted":
		return "interrupted; worktree restored: " + restored()
	case "error":
		first := "no reason given"
		if e.Error != nil {
			for _, l := range strings.Split(*e.Error, "\n") {
				if strings.TrimSpace(l) != "" {
					first = l
					break
				}
			}
		}
		return "failed: " + Plain(first)
	}
	return Plain(*e.Outcome)
}

// response handles an answer to a command this model sent. A refused
// prompt hands input back: no run started.
func (m *Model) response(r *frames.ResponseFields) {
	ok := r.Success != nil && *r.Success
	cmd := ""
	if r.Command != nil {
		cmd = *r.Command
	}
	if ok {
		switch m.answered(r) {
		case forState:
			m.stateAnswered(r.Data)
			return
		case forModel:
			m.modelAnswered(r.Data)
			return
		case forPaths:
			m.pathsAnswered(*r.ID, r.Data)
			return
		case forCodebase:
			m.codebaseAnswered(r.Data)
			return
		case forRadius:
			m.radiusAnswered(r.Data)
			return
		case forClassify:
			m.classified(r, true)
			return
		}
		if cmd == "set_model" && r.ID != nil && *r.ID == m.setModelID {
			// A model is chosen: its tier can now be asked for.
			m.refreshState()
			return
		}
	} else {
		switch m.answered(r) {
		case forState:
			return
		case forClassify:
			m.classified(r, false)
			return
		}
	}
	switch cmd {
	case "steer":
		m.steerAnswered(r, ok)
		return
	case "abort":
		if ok {
			m.abortAnswered(r.Data)
			return
		}
	}
	if ok {
		return
	}
	why := "no reason given"
	if r.Error != nil && strings.TrimSpace(*r.Error) != "" {
		why = *r.Error
	}
	if cmd == "prompt" && r.ID != nil && *r.ID == m.promptID {
		m.Running = false
		m.promptID = ""
	}
	if cmd == "" {
		cmd = "a command"
	}
	m.pushLine("! " + Plain(cmd) + ": " + Plain(firstLine(why)))
}

// exited ends the program. A child that exits without having been asked to
// is said to have, and where the worktree stands is not claimed: only
// trellis knew, and it has gone.
func (m *Model) exited(err error) {
	if !m.Quitting {
		m.pushLine("! trellis rpc exited unexpectedly" + exitReason(err))
		if m.Running {
			m.pushLine("! whether the run in flight rolled back is not known here; check `git status` before rerunning")
		}
	}
	m.Running = false
	m.Modal = nil
	m.approvals = nil
	m.Done = true
}

func exitReason(err error) string {
	if err == nil {
		return ""
	}
	return ": " + Plain(err.Error())
}

func firstLine(s string) string {
	for _, l := range strings.Split(s, "\n") {
		if strings.TrimSpace(l) != "" {
			return l
		}
	}
	return s
}

// pushText adds served text as lines, skipping blank ones: spacing lines
// carry nothing and would push what does off the screen.
func (m *Model) pushText(s string) {
	for _, l := range strings.Split(s, "\n") {
		if strings.TrimSpace(l) == "" {
			continue
		}
		m.pushLine(Plain(l))
	}
}

// pushLine adds one whole line and keeps the cap.
func (m *Model) pushLine(l string) {
	m.Transcript = append(m.Transcript, l)
	if over := len(m.Transcript) - TranscriptCap; over > 0 {
		m.Transcript = append([]string(nil), m.Transcript[over:]...)
	}
}

// Plain removes control characters (C0, DEL and C1, ESC among them) and
// line breaks, so served text can never move the cursor, recolour the
// screen or start an escape sequence in the terminal.
func Plain(s string) string {
	if strings.IndexFunc(s, unicode.IsControl) < 0 {
		return s
	}
	return strings.Map(func(r rune) rune {
		if unicode.IsControl(r) {
			return -1
		}
		return r
	}, s)
}

// View draws the screen: the six regions, or the approval modal over them.
func (m *Model) View() string {
	w, h := m.Width, m.Height
	if w <= 0 || h <= 0 {
		w, h = view.MinWidth, view.MinHeight
	}
	if m.Modal != nil {
		return view.Modal(m.Modal.Question, m.Modal.Diff, m.Modal.Scroll, w, h)
	}
	return view.Render(m.viewState(), w, h)
}
