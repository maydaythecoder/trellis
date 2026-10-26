package model

import (
	"fmt"
	"strconv"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The run controls (#857), the desktop's #837 and #838 for the TUI: pause
// and resume, raising a limit, taking a queued item back, and editing the
// model's plan (plan.go). Each is a control key, since every printable key
// is the task line's: ^P pause or resume, ^R raise, ^T take back, ^E edit
// the plan. None of them is ^S or ^Q, which a terminal with flow control
// keeps, or ^B, tmux's prefix.
//
// What each one shows comes from trellis, never from the key: the run is
// paused only from the engine's paused event; a raised ceiling is drawn
// only from the run budget trellis sends again; a taken-back item's text
// returns to the task line only on trellis's success answer to the
// withdraw.
//
// A raise and a plan edit borrow the task line: what it held is put aside
// and put back when the raise is sent or the edit is, when either is
// cancelled with ESC, and when the run ends.

// controlKey takes the control keys, and the keys of a raise or a plan
// edit while one is open, and reports whether it took k.
func (m *Model) controlKey(k tea.KeyMsg) bool {
	switch {
	case m.raising != "" && m.raiseKey(k):
		return true
	case m.planEdit != nil && m.planEditKey(k):
		return true
	}
	switch k.Type {
	case tea.KeyCtrlP:
		m.pauseOrResume()
	case tea.KeyCtrlR:
		m.cycleRaise()
	case tea.KeyCtrlT:
		m.takeBack()
	case tea.KeyCtrlE:
		m.openPlanEdit()
	default:
		return false
	}
	return true
}

// controllable is whether a control can reach the run now: one is in
// flight and not stopping, since trellis refuses a pause, a raise or a
// steer with no run, and a stopping run is rolling back.
func (m *Model) controllable() bool { return m.Running && !m.strip.Stopping }

// pauseOrResume is ^P: pause, or resume once a pause is asked or held. A
// pause asked is one trellis counts already, and a resume withdraws it
// before the hold is reached (ADR 0058 section 1), so resume is what the
// key offers from the moment pause is sent; the strip still says RUNNING
// until the paused event.
func (m *Model) pauseOrResume() {
	if !m.controllable() {
		m.push(view.RoleNote, "· nothing to pause: no run is in flight")
		return
	}
	if m.paused || m.pauseAsked {
		m.send("resume", nil)
		return
	}
	m.pauseAsked = true
	m.send("pause", nil)
}

// pausedEvent is the loop's paused event: the run is held before the
// model call for turn. Only this says so.
func (m *Model) pausedEvent(turn *uint64) {
	if !m.Running {
		return
	}
	m.paused, m.pauseAsked = true, false
	m.pausedAt = m.now()
	if turn != nil {
		m.push(view.RoleNote, fmt.Sprintf("· paused before the model call for turn %d", *turn))
	} else {
		m.push(view.RoleNote, "· paused before the next model call")
	}
}

// resumedEvent is the loop's resumed event: the held run goes on, held for
// pausedMs by trellis's clock.
func (m *Model) resumedEvent(pausedMs *uint64) {
	if !m.paused {
		return
	}
	m.closePause()
	if pausedMs != nil {
		m.push(view.RoleNote, fmt.Sprintf("· resumed after a hold of %.1f s", float64(*pausedMs)/1000))
	} else {
		m.push(view.RoleNote, "· resumed")
	}
}

// closePause ends a hold, counting it into the time this view's clock
// leaves out, as trellis leaves it out of elapsed_ms.
func (m *Model) closePause() {
	if m.paused && !m.pausedAt.IsZero() {
		m.waited += m.now().Sub(m.pausedAt)
	}
	m.paused, m.pausedAt = false, time.Time{}
}

// controlsEnded is a run's end, its child's, or a new run's start: a hold
// ends with its run, an abort ending a held run with no resumed event; a
// pause still asked is answered no more; and a raise or a plan edit has no
// run to reach.
func (m *Model) controlsEnded() {
	m.closePause()
	m.pauseAsked = false
	m.endModes()
}

// endModes closes a raise or a plan edit, putting back what the task line
// held when it opened.
func (m *Model) endModes() {
	if m.raising != "" || m.planEdit != nil {
		m.Input = m.stash
	}
	m.raising, m.planEdit, m.planOrig, m.stash = "", nil, nil, ""
}

// borrowLine puts the task line aside for a raise or a plan edit, and
// gives it text.
func (m *Model) borrowLine(text string) {
	m.stash, m.Input = m.Input, text
	m.Offered = nil
}

// cycleRaise is ^R: the next budget row to raise, from none to the turns,
// the wall clock and the completion tokens, then none again. Only the
// three ceilings set_budget takes, never the context (#807).
func (m *Model) cycleRaise() {
	if m.planEdit != nil {
		return
	}
	if !m.controllable() {
		m.push(view.RoleNote, "· nothing to raise: no run is in flight")
		return
	}
	next := view.Limits[0]
	for i, l := range view.Limits {
		if l == m.raising {
			if i+1 == len(view.Limits) {
				m.endModes()
				return
			}
			next = view.Limits[i+1]
		}
	}
	if m.raising == "" {
		m.borrowLine("")
	} else {
		m.Input = ""
	}
	m.raising = next
}

// raiseKey takes a raise's keys: ⏎ sends it, ESC cancels it, ^R moves to
// the next row, and Tab, which would change the verb or the pane under it,
// does nothing.
func (m *Model) raiseKey(k tea.KeyMsg) bool {
	switch k.Type {
	case tea.KeyEnter:
		m.sendRaise()
	case tea.KeyEsc:
		m.endModes()
	case tea.KeyTab, tea.KeyShiftTab:
	default:
		return false
	}
	return true
}

// sendRaise sends the typed ceiling as set_budget, once it reads as one.
// The budget row's ceiling is not changed here, and set_budget's answer is
// not read into it: trellis sends the run budget again where the raise
// takes effect, and that is what the row draws (#807). A raise refused
// here stays on the task line, with why.
func (m *Model) sendRaise() {
	field, value, why := raiseValue(m.raising, strings.TrimSpace(m.Input), m.Budget)
	if why != "" {
		m.push(view.RoleNote, "· not sent: "+why)
		return
	}
	m.send("set_budget", map[string]any{field: value})
	typed := strings.TrimSpace(m.Input) + " " + m.raising.Unit()
	m.push(view.RoleNote, fmt.Sprintf("· asked trellis to raise the %s ceiling to %s; the budget shows it once trellis sends the run budget again", m.raising, typed))
	m.endModes()
}

// maxIterations is the most turns set_budget takes, and maxExact the
// largest figure a JSON number carries exactly.
const (
	maxIterations = 4294967295
	maxExact      = 1 << 53
)

// raiseValue reads text as a raise of l: the set_budget field and its value
// in the field's units, or why it is not one. A ceiling is a whole number
// above 0, typed in l's unit (the wall clock in seconds, sent as
// milliseconds), above the ceiling drawn when there is one, as the
// desktop's page refuses (#837): trellis refuses one below the ceiling in
// force too, and its ceiling may be ahead of the one drawn.
func raiseValue(l view.Limit, text string, b view.Budget) (field string, value uint64, why string) {
	if text == "" || strings.Trim(text, "0123456789") != "" {
		return "", 0, fmt.Sprintf("a ceiling is a whole number of %s, typed as digits", l.Unit())
	}
	n, err := strconv.ParseUint(text, 10, 64)
	if err != nil || n > maxExact {
		return "", 0, "that ceiling is larger than trellis takes"
	}
	if n == 0 {
		return "", 0, "a ceiling is a whole number above 0"
	}
	var ceiling *uint64
	per := uint64(1)
	switch l {
	case view.LimitTurns:
		if n > maxIterations {
			return "", 0, fmt.Sprintf("trellis takes at most %d turns", uint64(maxIterations))
		}
		field, ceiling = "max_iterations", b.MaxIterations
	case view.LimitWallClock:
		if n > maxExact/1000 {
			return "", 0, "that ceiling is larger than trellis takes"
		}
		field, ceiling, per = "max_wall_clock_ms", b.MaxWallClockMs, 1000
	default:
		field, ceiling = "max_completion_tokens", b.MaxCompletionTokens
	}
	value = n * per
	if ceiling != nil && value <= *ceiling {
		return "", 0, fmt.Sprintf("%d %s is not above the ceiling in force, %d %s", n, l.Unit(), *ceiling/per, l.Unit())
	}
	return field, value, ""
}

// takeBack is ^T: a withdraw for the newest steer or follow-up of the run
// in flight that is still queued, sent with that item's own id, as rpc
// reads a withdraw's (#808). Nothing changes here but that the item waits
// on the answer: only trellis's success takes it back (takenBack).
func (m *Model) takeBack() {
	if !m.Running {
		m.push(view.RoleNote, "· nothing to take back: no run is in flight")
		return
	}
	i := m.withdrawable()
	if i < 0 {
		m.push(view.RoleNote, "· nothing to take back: no steer or follow-up of this run is still queued")
		return
	}
	q := &m.queue[i]
	q.item.Withdrawing = true
	m.outbox = append(m.outbox, Out{Cmd: map[string]any{"id": q.id, "type": "withdraw"}})
}

// withdrawable is the newest item ^T would take back, or -1: a steer or
// follow-up trellis listed, still queued, with no withdraw waiting. A Stop
// and send's held line is this view's, not trellis's, so no withdraw names
// it.
func (m *Model) withdrawable() int {
	for i := len(m.queue) - 1; i >= 0; i-- {
		q := m.queue[i]
		if (q.item.Kind == kindSteer || q.item.Kind == kindFollowUp) && q.id != "" &&
			q.item.Fate == view.FateQueued && !q.item.Withdrawing {
			return i
		}
	}
	return -1
}

// withdrawSettled is a withdraw's refusal, or its unwritten line: the item
// waits on it no more, and stands where it did.
func (m *Model) withdrawSettled(id *string) {
	if id == nil {
		return
	}
	for i := range m.queue {
		if m.queue[i].id == *id && m.queue[i].item.Withdrawing {
			m.queue[i].item.Withdrawing = false
		}
	}
}

// takenBack is trellis's success answer to a withdraw: the item is
// withdrawn, and its text goes back on the task line, after anything typed
// there, with the verb set to what it was sent as. The answer settles it
// whatever this view last made of it, since the withdrawn steer's own
// answer, a refusal, may be read first. A steer taken back is told, so
// that refusal, read after, says nothing more.
func (m *Model) takenBack(r *frames.ResponseFields) {
	if r.ID == nil {
		return
	}
	id := *r.ID
	kind := ""
	if d, ok := r.Data.(map[string]any); ok {
		if t, isStr := d["type"].(string); isStr && (t == kindSteer || t == kindFollowUp) {
			kind = t
		}
	}
	at := -1
	for i, q := range m.queue {
		if q.id == id && q.item.Kind != kindPrompt && (kind == "" || kind == q.item.Kind) &&
			(q.item.Fate == view.FateQueued || q.item.Fate == view.FateNot) {
			at = i
			break
		}
	}
	if at < 0 {
		return
	}
	q := &m.queue[at]
	q.item.Fate, q.item.Withdrawing, q.item.Why = view.FateWithdrawn, false, ""
	text := q.sent
	if q.item.Kind == kindSteer {
		// The steer as this view sent it, whole, rather than trellis's
		// listing of it.
		if i := m.steerIndex(&id); i >= 0 {
			text = m.steers[i].text
			m.steers[i].told, m.steers[i].why = true, "taken back"
		}
	}
	back := pasted([]rune(text))
	// A raise or a plan edit has the line: the text joins what it put aside.
	line := &m.Input
	if m.raising != "" || m.planEdit != nil {
		line = &m.stash
	}
	if strings.TrimSpace(*line) == "" {
		*line = back
	} else {
		*line += " " + back
	}
	if q.item.Kind == kindFollowUp {
		m.verb = view.VerbQueue
	} else {
		m.verb = view.VerbSteer
	}
	m.push(view.RoleNote, "· taken back: "+view.Plain(back)+"; it is on the task line again")
}
