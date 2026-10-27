package model

import (
	"strings"
	"time"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/ring"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The panes are drawn from the same turn, run budget, run_start and run_end
// frames the desktop reads, so the two clients draw one run the same way.
// Nothing here computes a verdict: a check row comes only from a verdict
// trellis sent (ADR 0004).

// clearPanes empties the run panes for a new run. They are kept after a run
// ends, so what it did stays on screen until the next one starts. The last
// run's restored goes with them: a run starts from a clean tree, and the
// last run's ending is not this one's (#636).
func (m *Model) clearPanes() {
	// A new run has no hold, no raise and no plan of its own yet; the last
	// one's hold is closed before this run's clock starts.
	m.controlsEnded()
	m.emptyPanes()
	m.Radius = nil
	m.Budget = view.Budget{}
	m.restored = nil
	m.runStart = m.now()
	m.waited = 0
	m.engineMs, m.engineMark = nil, 0
	m.strip.Phase, m.strip.Call = view.Phase{}, view.Call{}
	m.strip.Stopping = false
	m.strip.Outcome, m.strip.Exit = "", nil
	m.runID = ""
	m.closing = closing{}
	m.plan = view.Plan{}
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
	// Every turn is an action, whatever it was and however it went: a run
	// that tried four times and wrote once is the run the graph exists to
	// show. Only an applied edit, or a file write_file created, changed the
	// tree, and the event's wrote says which, as the trail records it
	// (#800). Read from the call and observation instead, the live view and
	// the trail could disagree about what a turn touched. A turn whose event
	// carried no wrote is not counted as one.
	wrote := t.Wrote != nil && *t.Wrote
	tool, target := "", ""
	if t.Tool != nil {
		tool = *t.Tool
	}
	if t.Target != nil {
		target = *t.Target
	}
	m.Actions.Push(view.Action{Tool: tool, Target: target, Wrote: wrote})
	m.strip.Phase = view.Phase{Kind: "tool", Turn: t.Turn, Tool: tool, Target: target}
	// Kept past the next model call and run_end, which clear the phase:
	// Where it is names the last call the run made, not only one under way.
	m.strip.Call = view.Call{Tool: tool, Target: target}

	// A refused edit is not a proposal at all: listing one made the panes
	// contradict each other ("3 edit(s) pending" for a run that changed one
	// line). Only an edit or a write_file sets wrote, and both target the
	// path they wrote.
	if wrote {
		m.Edits.Push(view.Edit{Path: target, Status: view.Proposed, Turn: t.Turn,
			Added: t.LinesAdded, Removed: t.LinesRemoved, Diff: t.Diff})
	}

	// A check row only from Some(true) or Some(false): None is a turn that
	// ran no check, and anything else is not a verdict trellis sent.
	if t.Verdict != nil && (*t.Verdict == "Some(true)" || *t.Verdict == "Some(false)") {
		name := "check"
		if t.CheckKind != nil && strings.TrimSpace(*t.CheckKind) != "" {
			name = *t.CheckKind
		}
		m.Checks.Push(view.Check{Name: name, Passed: *t.Verdict == "Some(true)", Turn: t.Turn, TookMs: t.DurationMs})
	}

	if t.Resolved != nil && t.Candidates != nil && t.Unresolved != nil {
		m.Radius = &view.Radius{Resolved: *t.Resolved, Candidates: *t.Candidates, Unresolved: *t.Unresolved}
	}

	m.reportedElapsed(t.ElapsedMs)
	elapsed := uint64(m.elapsed() / time.Second)
	m.Budget = view.Budget{
		Iteration:        t.Turn,
		MaxIterations:    m.Budget.MaxIterations,
		PromptBytes:      t.PromptBytes,
		ElapsedSecs:      &elapsed,
		CompletionTokens: t.CompletionTokens,
		ModelMs:          t.ModelMs,
		QueueWaitMs:      t.QueueWaitMs,
		// The ceilings stay the run budget's; a turn carries none.
		MaxWallClockMs:      m.Budget.MaxWallClockMs,
		MaxCompletionTokens: m.Budget.MaxCompletionTokens,
		MaxContextBytes:     m.Budget.MaxContextBytes,
		TokensUsed:          t.CompletionTokensUsed,
	}
}

// runBudget is the run's ceilings, from the loop's run budget event. The
// loop sends it again, whole, where a set_budget raise takes effect (#807),
// so it replaces every ceiling, one it lacks included, and leaves what the
// run has counted so far: replacing the budget whole emptied the turn, the
// time and the tokens mid-run (#857).
func (m *Model) runBudget(f frames.Frame) {
	m.Budget.MaxIterations, m.Budget.MaxWallClockMs = f.MaxIterations, f.MaxWallClockMs
	m.Budget.MaxCompletionTokens, m.Budget.MaxContextBytes = f.MaxCompletionTokens, f.MaxContextBytes
}

// tick keeps the budget's elapsed time current between turns (#641): the
// screen moved only when a turn arrived, and a long model call read as a
// hang. The figure is elapsed's; a tick with no run in flight changes
// nothing, so a run that has ended keeps the time it took. This view's own
// clock says nothing before its first second, while trellis's figure is
// drawn from the first one it sends.
func (m *Model) tick() {
	if !m.Running || m.runStart.IsZero() {
		return
	}
	if d := m.elapsed(); m.engineMs != nil || d >= time.Second {
		secs := uint64(d / time.Second)
		m.Budget.ElapsedSecs = &secs
	}
}

// reportedElapsed takes an elapsed_ms trellis sent (#906) as the figure the
// strip counts on from; nil, an event that carried none, changes nothing.
func (m *Model) reportedElapsed(ms *uint64) {
	if ms == nil {
		return
	}
	v := *ms
	m.engineMs, m.engineMark = &v, m.ownElapsed()
}

// elapsed is the run's time as the strip draws it: the last figure trellis
// sent, counted on since by this view's clock, which leaves out a question
// waiting on the person as trellis's budget does; or, with none sent, this
// view's own clock alone.
func (m *Model) elapsed() time.Duration {
	own := m.ownElapsed()
	if m.engineMs == nil {
		return own
	}
	return time.Duration(*m.engineMs)*time.Millisecond + max(own-m.engineMark, 0)
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
	// A held run is not charged for its hold, as trellis's elapsed_ms is
	// not (#806), so the count stops while it is paused (#857).
	if m.paused && !m.pausedAt.IsZero() {
		d -= now.Sub(m.pausedAt)
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
		Filter:     m.filter,
		Quiet:      m.quiet,
		CallOpen:   m.call != nil,
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
		Nav:        m.nav,
		Asked:      m.waiting(),
		Reviewing:  m.Modal != nil && m.Modal.Reviewing,
		Compose:    m.composeState(),
		Summary:    view.Summary{RunID: m.runID, Steers: m.steerItems(), Closing: m.closingState()},
		Playful:    m.playful,
		Raise:      m.raising,
		Plan:       m.planState(),
	}
}

// waiting is every question waiting on the person, the one on screen first,
// for the code level (#853).
func (m *Model) waiting() []view.Asked {
	var out []view.Asked
	if m.Modal != nil {
		out = append(out, view.Asked{ID: m.Modal.ID, Question: m.Modal.Question, Detail: m.Modal.Diff, OnScreen: true})
	}
	for _, a := range m.approvals {
		out = append(out, view.Asked{ID: a.ID, Question: a.Question, Detail: a.Diff})
	}
	return out
}

// nowState is the Now strip's and Where it is's state: what the model
// tracks, and whether a question waits on the person now.
func (m *Model) nowState() view.Now {
	n := m.strip
	n.Waiting = m.Modal != nil
	if m.Modal != nil {
		n.Question = m.Modal.Question
	}
	n.EngineClock = m.engineMs != nil
	n.Paused, n.PauseAsked = m.paused, m.pauseAsked
	// Before any prompt, the mode the next one names; after, the one the
	// last prompt sent, which a follow_up's run also runs in (ADR 0025).
	if n.Control == "" {
		n.Control = m.control
	}
	return n
}
