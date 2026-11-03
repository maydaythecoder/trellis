package view

import (
	"fmt"
	"strings"
)

// The plan level (#857), the terminal face of the desktop's Plan panel
// (#838; the Plan card; ADR 0058 section 5): the model's latest plan in
// the run, gone into from the Now strip's way in, the panes' row folded, or
// Tab. trellis sends a plan only where the repository enables the opt-in
// plan tool, so a run without one says that.
//
// The steps are the model's words (CLAUDE.md rule 10): made plain and drawn
// as text, never counted, and a plan the model could not read is said to
// be unreadable with its reason, never drawn in part. A person's edit goes
// back as an ordinary steer; nothing here edits the plan trellis holds,
// which changes only when the model sends a new one.

// PlanStep is one step as the model wrote it: its text and its state, one
// of todo, current and done.
type PlanStep struct {
	Text, State string
}

// Plan is what the plan level draws.
type Plan struct {
	// Seen is set once a plan event has come in this run.
	Seen bool
	// Turn is the turn the plan came from, nil when the event carried none.
	Turn *uint64
	// Steps are the plan's steps, in the model's order; Why, when set, is
	// why they could not be read, and Steps is then empty.
	Steps []PlanStep
	Why   string
	// StepsCut and TextsCut are how many steps trellis dropped past the
	// eighth and how many texts it shortened, nil when not sent.
	StepsCut, TextsCut *uint64
	// CanEdit is set while an edit can reach the run: a run in flight and
	// not stopping, and steps to edit.
	CanEdit bool
	// Edit is the person's edit while it is being typed, nil otherwise.
	Edit *PlanEdit
}

// PlanEdit is a plan being edited: one line a step, in the marks the steer
// sends, and the step the task line is editing, whose text Lines holds as
// typed so far.
type PlanEdit struct {
	Lines []string
	At    int
}

// PlanMark is how a step's state is written, on the level and in the
// steer: the terminal's [x] done, [>] current and [ ] to do.
func PlanMark(state string) string {
	switch state {
	case "done":
		return "[x]"
	case "current":
		return "[>]"
	}
	return "[ ]"
}

// fromTurn is " from turn N", or nothing when the event carried no turn.
func fromTurn(turn *uint64) string {
	if turn == nil {
		return ""
	}
	return fmt.Sprintf(" from turn %d", *turn)
}

// planRows is the level's rows at inner width w: the edit being typed, or
// the plan with what trellis cut and the way to edit it, or why there is
// none.
func planRows(s State, w int) []Row {
	p := s.Plan
	if e := p.Edit; e != nil {
		rows := wrapped(" ", "", "your edit of the model's plan"+fromTurn(p.Turn)+": it goes to the model as a steer", RoleYou, w)
		for i, l := range e.Lines {
			lead, role := "  ", RoleText
			if i == e.At {
				lead, role = "▸ ", RoleYou
			}
			rows = append(rows, wrapped(" ", lead, Plain(l), role, w)...)
		}
		return append(rows, Row{}, Row{Text: " ↑↓ step · ↓ past the last adds one · an empty step is left out", Role: RoleLabel})
	}
	switch {
	case !p.Seen:
		return wrapped(" ", "", `no plan in this run: the model sends one only when the repository enables the plan tool ([tools] enable = ["plan"])`, RoleLabel, w)
	case p.Why != "":
		return wrapped(" ", "", "the model's plan"+fromTurn(p.Turn)+" could not be read: "+Plain(p.Why), RoleNote, w)
	}
	var rows []Row
	if len(p.Steps) == 0 {
		rows = append(rows, Row{Text: " the model's plan has no steps", Role: RoleLabel})
	}
	for _, st := range p.Steps {
		rows = append(rows, wrapped(" ", PlanMark(st.State)+" ", Plain(st.Text), RoleText, w)...)
	}
	rows = append(rows, Row{})
	rows = append(rows, wrapped(" ", "", "the model's own account of its plan"+fromTurn(p.Turn)+", not a check's", RoleLabel, w)...)
	if p.StepsCut != nil && *p.StepsCut > 0 {
		rows = append(rows, wrapped(" ", "", fmt.Sprintf("trellis cut %d step(s) past the eighth: a plan that long is a sign the task should have been split", *p.StepsCut), RoleLabel, w)...)
	}
	if p.TextsCut != nil && *p.TextsCut > 0 {
		rows = append(rows, wrapped(" ", "", fmt.Sprintf("trellis shortened %d step text(s) to 200 characters", *p.TextsCut), RoleLabel, w)...)
	}
	if p.CanEdit {
		rows = append(rows, wrapped(" ", "", Keycap("^E")+" edit it: your version goes to the model as a steer", RoleLabel, w)...)
	}
	return rows
}

// planTitle is the level's title.
func planTitle(s State) string {
	if s.Plan.Edit != nil {
		return "plan · your edit"
	}
	return "plan"
}

// planKeys is the status bar's keys at the plan level.
func planKeys(s State) string {
	switch {
	case s.Plan.Edit != nil:
		return keyRow(key{"↑↓", "step"}, key{"⏎", "send"}, key{"ESC", "cancel"}, stopKey(s))
	case s.Plan.CanEdit:
		return keyRow(key{"ESC", "out"}, key{"^E", "edit"}, stopKey(s))
	}
	return keyRow(key{"ESC", "out"}, key{"↑↓", "scroll"}, stopKey(s))
}

// plan is the level at w by h, from row Nav.Scroll down, as the summary
// level scrolls.
func plan(s State, w, h int) []string {
	rows := planRows(s, w-2)
	innerH := max(h-2, 0)
	from := clamp(s.Nav.Scroll, 0, max(len(rows)-innerH, 0))
	bottom := ""
	if len(rows) > innerH {
		to := min(from+innerH, len(rows))
		bottom = fmt.Sprintf("rows %d–%d of %d", from+1, to, len(rows))
	}
	return Box(s.Theme, planTitle(s), bottom, rows[from:], w, h)
}

// PlanTop is the furthest the plan level scrolls at w by h, the view's own
// limit, as SummaryTop is the summary's.
func PlanTop(s State, w, h int) int {
	w, h = max(w, MinWidth), max(h, MinHeight)
	_, bodyH := layout(s, w, h)
	return max(len(planRows(s, w-railW-2))-max(bodyH-2, 0), 0)
}

// PlanSteer is the steer an edit of the plan sends: the desktop's words
// (#838), then the person's steps, one a line, so the model reads both what
// changed and that the person changed it.
func PlanSteer(turn *uint64, lines []string) string {
	head := "I edited your plan" + fromTurn(turn) + ". Work from this plan now, one step a line ([x] done, [>] current, [ ] to do):"
	return head + "\n" + strings.Join(lines, "\n")
}
