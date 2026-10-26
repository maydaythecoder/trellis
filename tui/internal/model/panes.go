package model

import (
	"strings"
	"time"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/ring"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The panes are drawn from the same turn, run budget, run_start and run_end
// frames the desktop reads, as tui_bridge.rs turned the loop's events into
// the ratatui TUI's messages. Nothing here computes a verdict: a check row
// comes only from a verdict trellis sent (ADR 0004).

// clearPanes empties the run panes for a new run. They are kept after a run
// ends, so what it did stays on screen until the next one starts. The last
// run's restored goes with them: a run starts from a clean tree, and the
// last run's ending is not this one's (#636).
func (m *Model) clearPanes() {
	m.emptyPanes()
	m.Radius = nil
	m.Budget = view.Budget{}
	m.restored = nil
	m.runStart = m.now()
	m.waited = 0
	m.strip.Phase = view.Phase{}
	m.strip.Stopping = false
	m.strip.Outcome, m.strip.Exit = "", nil
}

// emptyPanes makes the three lists a run fills, each keeping the newest
// PaneCap rows (caps.go).
func (m *Model) emptyPanes() {
	m.Actions = ring.New[view.Action](PaneCap)
	m.Edits = ring.New[view.Edit](PaneCap)
	m.Checks = ring.New[view.Check](PaneCap)
}

// panesTurn adds a turn to the panes.
func (m *Model) panesTurn(t *frames.TurnFields) {
	obs := ""
	if t.Observation != nil {
		obs = *t.Observation
	}
	// Every turn is an action, whatever it was and however it went: a run
	// that tried four times and wrote once is the run the graph exists to
	// show. Only an applied edit, or a file write_file created, changed the
	// tree; the observation is the only place that says so.
	call := t.Call
	wrote := (strings.HasPrefix(call, "edit ") && strings.Contains(obs, "applied: true")) ||
		(strings.HasPrefix(call, "write_file ") && strings.HasPrefix(obs, "created "))
	tool, target, _ := strings.Cut(call, " ")
	m.Actions.Push(view.Action{Tool: tool, Target: target, Wrote: wrote})
	m.strip.Phase = view.Phase{Kind: "tool", Turn: t.Turn, Tool: tool, Target: target}

	// A refused edit is not a proposal at all: listing one made the panes
	// contradict each other ("3 edit(s) pending" for a run that changed one
	// line).
	if wrote {
		if f := strings.Fields(call); len(f) > 1 {
			m.Edits.Push(view.Edit{Path: f[1], Status: view.Proposed})
		}
	}

	// A check row only from Some(true) or Some(false): None is a turn that
	// ran no check, and anything else is not a verdict trellis sent.
	if t.Verdict != nil && (*t.Verdict == "Some(true)" || *t.Verdict == "Some(false)") {
		name := "check"
		if t.CheckKind != nil && strings.TrimSpace(*t.CheckKind) != "" {
			name = *t.CheckKind
		}
		m.Checks.Push(view.Check{Name: name, Passed: *t.Verdict == "Some(true)"})
	}

	if t.Resolved != nil && t.Candidates != nil && t.Unresolved != nil {
		m.Radius = &view.Radius{Resolved: *t.Resolved, Candidates: *t.Candidates, Unresolved: *t.Unresolved}
	}

	elapsed := uint64(m.ownElapsed() / time.Second)
	m.Budget = view.Budget{
		Iteration:        t.Turn,
		MaxIterations:    m.Budget.MaxIterations,
		PromptBytes:      t.PromptBytes,
		ElapsedSecs:      &elapsed,
		CompletionTokens: t.CompletionTokens,
		ModelMs:          t.ModelMs,
		QueueWaitMs:      t.QueueWaitMs,
	}
}

// runBudget is the run's ceiling, from the loop's one run budget event: a
// new budget with nothing counted yet.
func (m *Model) runBudget(max *uint64) {
	m.Budget = view.Budget{MaxIterations: max}
}

// tick keeps the budget's elapsed time current between turns (#641): the
// screen moved only when a turn arrived, and a long model call read as a
// hang. The figure is this view's clock (ownElapsed); a tick with no run in
// flight changes nothing, so a run that has ended keeps the time it took.
func (m *Model) tick() {
	if !m.Running || m.runStart.IsZero() {
		return
	}
	if d := m.ownElapsed(); d >= time.Second {
		secs := uint64(d / time.Second)
		m.Budget.ElapsedSecs = &secs
	}
}

// ownElapsed is the run's time by this view's clock: from its start, less
// every stretch a question waited on the person, the one on screen
// included. trellis's budget leaves that time out as well (the loop's
// waited_on_human), so a diff read slowly does not read as a slow run
// (#850). It is this view's figure, and the Now strip says so.
func (m *Model) ownElapsed() time.Duration {
	now := m.now()
	d := now.Sub(m.runStart) - m.waited
	if m.Modal != nil && !m.askedAt.IsZero() {
		d -= now.Sub(m.askedAt)
	}
	return max(d, 0)
}

// closeModal takes the question off the screen, and counts the time it
// waited on the person into waited.
func (m *Model) closeModal() {
	if m.Modal != nil && !m.askedAt.IsZero() {
		m.waited += m.now().Sub(m.askedAt)
	}
	m.Modal, m.askedAt = nil, time.Time{}
}

// showModal puts a question on the screen and starts the time it waits on
// the person.
func (m *Model) showModal(a *Approval) {
	m.Modal, m.askedAt = a, m.now()
}

// viewState is what the screen draws.
func (m *Model) viewState() view.State {
	return view.State{
		Model:      m.ModelName,
		Tier:       m.Tier,
		Transcript: m.Transcript,
		ScrollBack: m.ScrollBack,
		Outcome:    m.Outcome,
		Running:    m.Running,
		Input:      m.Input,
		Offered:    m.Offered,
		Edits:      m.Edits.Slice(),
		Checks:     m.Checks.Slice(),
		Actions:    m.Actions.Slice(),
		Dropped:    view.Dropped{Actions: m.Actions.Dropped(), Edits: m.Edits.Dropped(), Checks: m.Checks.Dropped()},
		Radius:     m.Radius,
		Budget:     m.Budget,
		Stderr:     m.Stderr,
		Restored:   m.restored,
		Theme:      m.theme,
		Now:        m.nowState(),
	}
}

// nowState is the Now strip's and Where it is's state: what the model
// tracks, and whether a question waits on the person now.
func (m *Model) nowState() view.Now {
	n := m.strip
	n.Waiting = m.Modal != nil
	return n
}
