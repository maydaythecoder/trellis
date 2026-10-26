package model

import (
	"encoding/json"
	"fmt"
	"strings"
	"unicode/utf8"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The model's plan (#857), read as the desktop's reducer reads it (#838):
// the run's latest plan event, its steps string parsed here and refused
// whole, with why, when it is not the plan trellis keeps. The steps are the
// model's words: kept as text, made plain where drawn, never counted.
//
// An edit is typed a step at a time on the task line, ↑ and ↓ moving
// between steps, and goes back as one ordinary steer in the desktop's
// words (view.PlanSteer); the plan drawn changes only when the model sends
// a new one.

// The plan trellis keeps (ADR 0058 section 5): at most PlanStepsMax steps
// of at most PlanTextChars characters; a steps string past PlanStepsBytes,
// about twice what that much JSON takes, is not one it kept. PlanEditCap is
// the most steps an edit holds, ↓ past the last adding one.
const (
	PlanStepsBytes = 16 * 1024
	PlanStepsMax   = 8
	PlanTextChars  = 200
	PlanEditCap    = 16
)

// planEvent keeps a plan event, replacing the last whole.
func (m *Model) planEvent(f *frames.PlanFields) {
	if f == nil {
		return
	}
	steps, why := planSteps(f.Steps)
	m.plan = view.Plan{Seen: true, Turn: f.Turn, Steps: steps, Why: why, StepsCut: f.StepsCut, TextsCut: f.TextsCut}
}

// planSteps reads a plan event's steps string as the contract says it is:
// a JSON array of at most PlanStepsMax {text, state} objects, each text a
// string of at most PlanTextChars characters and each state todo, current
// or done. Anything else is unreadable, with why, and none of it is a step.
func planSteps(steps *string) ([]view.PlanStep, string) {
	if steps == nil {
		return nil, "the plan event carried no steps string"
	}
	if len(*steps) > PlanStepsBytes {
		return nil, fmt.Sprintf("its steps are %d bytes, past the %d this view reads", len(*steps), PlanStepsBytes)
	}
	var list []any
	if err := json.Unmarshal([]byte(*steps), &list); err != nil {
		var other any
		if json.Unmarshal([]byte(*steps), &other) == nil {
			return nil, "its steps are not a JSON array"
		}
		return nil, "its steps are not JSON: " + err.Error()
	}
	if list == nil {
		return nil, "its steps are not a JSON array"
	}
	if len(list) > PlanStepsMax {
		return nil, fmt.Sprintf("it carries %d steps, and trellis keeps at most %d", len(list), PlanStepsMax)
	}
	out := make([]view.PlanStep, 0, len(list))
	for i, s := range list {
		n := i + 1
		o, _ := s.(map[string]any)
		text, isStr := o["text"].(string)
		if !isStr {
			return nil, fmt.Sprintf("step %d has no text", n)
		}
		if utf8.RuneCountInString(text) > PlanTextChars {
			return nil, fmt.Sprintf("step %d's text is longer than the %d characters trellis keeps", n, PlanTextChars)
		}
		state, _ := o["state"].(string)
		if state != "todo" && state != "current" && state != "done" {
			return nil, fmt.Sprintf("step %d's state is not todo, current or done", n)
		}
		out = append(out, view.PlanStep{Text: text, State: state})
	}
	return out, ""
}

// planEditable is whether an edit can reach the run now: a run in flight
// and not stopping, and a plan with steps read.
func (m *Model) planEditable() bool {
	return m.controllable() && m.plan.Seen && m.plan.Why == "" && len(m.plan.Steps) > 0
}

// planLines is the plan as an edit starts from it, one step a line in the
// marks the steer sends.
func (m *Model) planLines() []string {
	out := make([]string, len(m.plan.Steps))
	for i, s := range m.plan.Steps {
		out[i] = view.PlanMark(s.State) + " " + view.Plain(s.Text)
	}
	return out
}

// openPlanEdit is ^E at the plan level: the plan's steps become the edit,
// its first step on the task line.
func (m *Model) openPlanEdit() {
	if m.nav.Open != view.PanelPlan || m.raising != "" || m.planEdit != nil {
		return
	}
	if !m.planEditable() {
		m.push(view.RoleNote, "· nothing to edit: an edit reaches only a run in flight whose plan was read")
		return
	}
	lines := m.planLines()
	m.planOrig = append([]string(nil), lines...)
	m.planEdit = &view.PlanEdit{Lines: lines}
	m.borrowLine(lines[0])
}

// planEditKey takes an edit's keys: ↑ and ↓ move between steps, ↓ past
// the last adding one; ⏎ sends the edit; ESC cancels it; Tab, which would
// change the verb or the pane under it, does nothing.
func (m *Model) planEditKey(k tea.KeyMsg) bool {
	e := m.planEdit
	switch k.Type {
	case tea.KeyUp, tea.KeyDown:
		e.Lines[e.At] = m.Input
		switch {
		case k.Type == tea.KeyUp:
			e.At = max(e.At-1, 0)
		case e.At+1 < len(e.Lines):
			e.At++
		case strings.TrimSpace(m.Input) != "" && len(e.Lines) < PlanEditCap:
			e.Lines = append(e.Lines, "")
			e.At++
		}
		m.Input = e.Lines[e.At]
	case tea.KeyEnter:
		m.sendPlanEdit()
	case tea.KeyEsc:
		m.endModes()
	case tea.KeyTab, tea.KeyShiftTab, tea.KeyCtrlE:
	default:
		return false
	}
	return true
}

// sendPlanEdit sends the edit as one steer, its empty steps left out. An
// edit with no step, or one that changes nothing, is not sent, and stays
// open; so does one past this run's steers cap, as a steer typed then
// stays on the task line.
func (m *Model) sendPlanEdit() {
	e := m.planEdit
	e.Lines[e.At] = m.Input
	var lines []string
	for _, l := range e.Lines {
		if l = strings.TrimSpace(l); l != "" {
			lines = append(lines, l)
		}
	}
	switch {
	case len(lines) == 0:
		m.push(view.RoleNote, "· not sent: type the plan, one step a line")
		return
	case strings.Join(lines, "\n") == strings.Join(m.planOrig, "\n"):
		m.push(view.RoleNote, "· not sent: change a step first; an unchanged plan is not sent")
		return
	case len(m.steers) >= SteersCap:
		m.push(view.RoleNote, fmt.Sprintf("· not sent: this run has %d steers already; the edit stays open", len(m.steers)))
		return
	}
	text := view.PlanSteer(m.plan.Turn, lines)
	// Its own rows, a step a row: the steer row's one line would run the
	// steps together.
	head := "· queued: your edit of the plan, as a steer"
	if m.plan.Turn != nil {
		head = fmt.Sprintf("· queued: your edit of the plan from turn %d, as a steer", *m.plan.Turn)
	}
	m.push(view.RoleYou, head)
	for _, l := range lines {
		m.push(view.RoleYou, "    "+view.Plain(l))
	}
	id := m.send("steer", map[string]any{"message": text})
	m.steers = append(m.steers, steer{text: text, id: id})
	m.refreshState()
	m.endModes()
}

// planState is what the plan level draws: the plan, whether it can be
// edited now, and the edit, its step on the task line as typed so far.
func (m *Model) planState() view.Plan {
	p := m.plan
	p.CanEdit = m.planEditable()
	if m.planEdit != nil {
		e := view.PlanEdit{Lines: append([]string(nil), m.planEdit.Lines...), At: m.planEdit.At}
		e.Lines[e.At] = m.Input
		p.Edit = &e
	}
	return p
}
