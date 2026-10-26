package model

import (
	"fmt"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// A steer is a line typed while a run is in flight: text for that run's
// next model request (ADR 0018, #115), never a second task, since a second
// run would start against a worktree the first is still editing.
//
// A steer is only text the model was shown. Whatever it says, it touches no
// check and no edit, so nothing here can read as the run having converged
// on it (ADR 0004). What it must not be is lost: each one is said to have
// been delivered at the turn that carried it, or not delivered when the run
// ended first, so the person knows to type it again.
type steer struct {
	text string
	// id is the steer command's id, to match trellis's answer to it.
	id string
	// turn is the turn that carried it; 0 until delivered.
	turn int
	// told is set once its line, delivered or not, has been written.
	told bool
}

// newRun forgets the last run's steers: they were counted per run, and one
// left over must not take this run's slot.
func (m *Model) newRun() {
	m.steers = nil
	m.deliveries = 0
}

// steer sends a line typed mid-run as `steer {message}`. A /command is
// refused, with the Rust TUI's wording, and nothing is sent: it is
// addressed to this view, and sending it to the model would surprise
// either way. A run that has SteersCap steers takes no more (#848): the
// line stays on the task line, unsent, and the transcript says why, so
// nothing the person typed is lost.
func (m *Model) steer(text string) {
	if text[0] == '/' {
		m.push(view.RoleNote, "· a run is already in flight; wait for it to finish")
		return
	}
	if len(m.steers) >= SteersCap {
		m.push(view.RoleNote, fmt.Sprintf("· not sent: this run has %d steers already; the line stays on the task line", len(m.steers)))
		return
	}
	m.Input = ""
	// The person's own words, so the row is theirs; whether it reached the
	// model is said by the notes below.
	m.push(view.RoleYou, "· queued: "+view.Plain(text))
	id := m.send("steer", map[string]any{"message": text})
	m.steers = append(m.steers, steer{text: text, id: id})
}

// delivered marks the nth steer of this run delivered at turn, once.
func (m *Model) delivered(nth, turn int) {
	if nth < 0 || nth >= len(m.steers) || m.steers[nth].told {
		return
	}
	s := &m.steers[nth]
	s.turn = turn
	s.told = true
	m.push(view.RoleNote, fmt.Sprintf("· delivered at turn %d: %s", turn, view.Plain(s.text)))
}

// notDelivered says, once, that the nth steer never reached the model.
func (m *Model) notDelivered(nth int) {
	if nth < 0 || nth >= len(m.steers) || m.steers[nth].told {
		return
	}
	m.steers[nth].told = true
	m.push(view.RoleNote, "· not delivered: "+view.Plain(m.steers[nth].text))
}

// steerDeliveredEvent is the loop's "steer delivered" event. It carries no
// text, so the steer is named by order: the loop drains its channel in the
// order sent and delivers a prefix of it, so the nth delivery is the nth
// steer sent. The turn is after_step + 1: after_step counts completed
// turns, and the steer is in the request that becomes the next one.
func (m *Model) steerDeliveredEvent(afterStep *uint64) {
	nth := m.deliveries
	m.deliveries++
	if afterStep == nil {
		return
	}
	m.delivered(nth, int(*afterStep)+1)
}

func (m *Model) steerIndex(id *string) int {
	if id == nil {
		return -1
	}
	for i, s := range m.steers {
		if s.id == *id {
			return i
		}
	}
	return -1
}

// steerAnswered is trellis's answer to one steer: success with the
// after_step of the request that carried it, or an error when the run
// ended before its next request.
func (m *Model) steerAnswered(r *frames.ResponseFields, ok bool) {
	i := m.steerIndex(r.ID)
	if i < 0 {
		return
	}
	if !ok {
		m.notDelivered(i)
		return
	}
	if d, isObj := r.Data.(map[string]any); isObj {
		if a, isNum := d["after_step"].(float64); isNum && a >= 0 && a == float64(int(a)) {
			m.delivered(i, int(a)+1)
		}
	}
}

// abortAnswered marks every steer abort handed back undelivered.
func (m *Model) abortAnswered(data any) {
	d, ok := data.(map[string]any)
	if !ok {
		return
	}
	list, _ := d["undelivered"].([]any)
	for _, u := range list {
		if e, isObj := u.(map[string]any); isObj {
			if id, isStr := e["id"].(string); isStr {
				m.notDelivered(m.steerIndex(&id))
			}
		}
	}
}

// steersNotDelivered says which of this run's steers never reached the
// model, as the run ends.
func (m *Model) steersNotDelivered() {
	for i := range m.steers {
		m.notDelivered(i)
	}
	m.steers = nil
	m.deliveries = 0
}
