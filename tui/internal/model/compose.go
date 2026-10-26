package model

import (
	"strings"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The composer (#852), the desktop's #830 for the TUI. While a run is in
// flight ⏎ sends the line under a verb: Steer, a steer for the run's next
// model request (steer.go); Queue, a follow_up trellis runs after this run;
// or Stop and send, an abort whose line is sent as a prompt once the run
// has ended. Tab cycles the verb while the line holds text and no @ word:
// an empty line keeps Tab for selecting a pane (#851), and an @ word for
// its completions. The verb goes back to Steer whenever a run starts or
// ends, so Stop and send, which rolls a run back, is never the verb a
// person finds already chosen.
//
// The queue lists what get_state's pending names (#801), oldest first,
// each queued until an event settles it: a steer's answer or its delivered
// event, a follow_up's or the held prompt's own run_start, an abort's
// undelivered, or the run ending first. An item leaving pending settles
// nothing, since a steer leaves it before its delivered event is written
// (#808). ^T takes the newest still queued back (controls.go, #857), and
// only trellis's success answer to the withdraw takes it off.

// Kinds of queued item.
const (
	kindSteer    = "steer"
	kindFollowUp = "follow_up"
	kindPrompt   = "prompt"
)

// queued is one item of the list and the id it was sent with. A held
// prompt's id is empty until the prompt is sent.
type queued struct {
	id   string
	item view.Queued
	// sent is the message as trellis listed it, whole up to SentCap, for a
	// take-back to put back on the task line (#857); item's text is cut
	// for the list.
	sent string
}

// held is the line a Stop and send holds. rpc writes the stopped run's
// run_end before it answers the abort, so at run_end whether the abort was
// refused is not yet known: the line is sent only once both have been
// read, ended and stopped, and never after a refused abort or the child's
// exit.
type held struct {
	abortID        string
	text           string
	ended, stopped bool
}

// composeTabs reports whether Tab changes the verb now.
func (m *Model) composeTabs() bool {
	if !m.Running || strings.TrimSpace(m.Input) == "" {
		return false
	}
	_, completing := m.atFragment()
	return !completing
}

// nextVerb is the verb after v, or before it going back, wrapping round.
func nextVerb(v view.Verb, back bool) view.Verb {
	n := len(view.Verbs)
	at := 0
	for i, w := range view.Verbs {
		if w == v {
			at = i
		}
	}
	if back {
		return view.Verbs[(at+n-1)%n]
	}
	return view.Verbs[(at+1)%n]
}

// compose sends a line typed while a run is in flight under the verb. A
// /command is refused as a steer refuses it, whatever the verb.
func (m *Model) compose(text string) {
	if text[0] == '/' {
		m.steer(text)
		return
	}
	switch m.verb {
	case view.VerbQueue:
		m.followUp(text)
	case view.VerbStop:
		m.stopAndSend(text)
	default:
		sent := len(m.steers)
		m.steer(text)
		if len(m.steers) > sent {
			m.refreshState()
		}
	}
}

// followUp sends the line as `follow_up {message}`: a task trellis runs
// once this run ends, accepted only while a run is in flight (#825), which
// is the only time it is sent. A refused one is said in the transcript as
// any refused command is, and never reaches pending.
func (m *Model) followUp(text string) {
	m.Input = ""
	m.push(view.RoleYou, "· queued after this run: "+view.Plain(text))
	m.send("follow_up", map[string]any{"message": text})
	m.refreshState()
}

// stopAndSend stops the run in flight and holds the line to send as a
// prompt once it has ended. One line is held at a time, since only one
// run can follow the stopped one: a second stays on the task line. The
// stop is ^C's abort, which asks for no confirmation either.
func (m *Model) stopAndSend(text string) {
	if m.held != nil {
		m.push(view.RoleNote, "· not sent: a stop and send already holds a line; this one stays on the task line")
		return
	}
	m.Input = ""
	m.push(view.RoleYou, "· stop and send: "+view.Plain(text))
	m.push(view.RoleNote, "· stopping: the run rolls back, then this line is sent as a new task")
	m.strip.Stopping = true
	m.held = &held{abortID: m.send("abort", nil), text: text}
	m.enqueue(queued{item: view.Queued{Kind: kindPrompt, Text: capLine(view.Plain(text))}})
}

// sendHeld sends the held line once the stopped run has ended and trellis
// has answered its abort with success, as the task it was typed as. It
// goes straight to prompt: the person chose to run it.
func (m *Model) sendHeld() {
	h := m.held
	if h == nil || !h.ended || !h.stopped || m.Quitting {
		return
	}
	m.held = nil
	m.push(view.RoleYou, "> "+view.Plain(h.text))
	m.startRun(h.text)
	if i := m.heldItem(); i >= 0 {
		m.queue[i].id = m.promptID
	}
}

// dropHeld says the held line will not be sent, and why. Nothing when no
// line is held.
func (m *Model) dropHeld(why string) {
	if m.held == nil {
		return
	}
	text := m.held.text
	m.held = nil
	if i := m.heldItem(); i >= 0 {
		m.queue[i].item.Fate, m.queue[i].item.Why = view.FateNot, why
	}
	m.push(view.RoleNote, "· not delivered: "+view.Plain(text)+" ("+view.Plain(why)+")")
}

// heldItem is the held prompt's place in the queue, or -1: the one prompt
// item not yet sent.
func (m *Model) heldItem() int {
	for i, q := range m.queue {
		if q.item.Kind == kindPrompt && q.id == "" && q.item.Fate == view.FateQueued {
			return i
		}
	}
	return -1
}

// enqueue adds an item, keeping the list to QueueCap: the oldest settled
// item makes room, and with none settled the new one is not listed (it is
// still sent, and its transcript row still says what became of it).
func (m *Model) enqueue(q queued) {
	if len(m.queue) >= QueueCap {
		at := -1
		for i, o := range m.queue {
			if o.item.Fate != view.FateQueued {
				at = i
				break
			}
		}
		if at < 0 {
			return
		}
		m.queue = append(m.queue[:at], m.queue[at+1:]...)
	}
	m.queue = append(m.queue, q)
}

// settle moves the first queued item of kind sent with id to fate.
func (m *Model) settle(kind, id string, fate view.Fate, after int, why string) {
	if id == "" {
		return
	}
	for i := range m.queue {
		q := &m.queue[i]
		if q.item.Kind == kind && q.id == id && q.item.Fate == view.FateQueued {
			q.item.Fate, q.item.After, q.item.Why = fate, after, why
			return
		}
	}
}

// pendingListed reads get_state's pending: each entry not yet listed joins
// the queue, as queued, or, for a steer whose answer was read first, where
// its transcript row says it stands. An entry already listed keeps its
// state, and one missing keeps its own. An entry without a non-empty
// string id, a listed type and a string message is not drawn: this client
// sends each with an id, so one without is not its own.
func (m *Model) pendingListed(d map[string]any) {
	list, _ := d["pending"].([]any)
	for _, p := range list {
		e, isObj := p.(map[string]any)
		if !isObj {
			continue
		}
		id, _ := e["id"].(string)
		kind, _ := e["type"].(string)
		msg, isStr := e["message"].(string)
		if id == "" || !isStr || (kind != kindSteer && kind != kindFollowUp) {
			continue
		}
		if m.listed(kind, id) {
			continue
		}
		item := view.Queued{Kind: kind, Text: capLine(view.Plain(oneLine(msg)))}
		if kind == kindSteer {
			if i := m.steerIndex(&id); i >= 0 && m.steers[i].told {
				s := m.steers[i]
				if s.turn > 0 {
					item.Fate, item.After = view.FateAfter, s.turn-1
				} else {
					item.Fate, item.Why = view.FateNot, s.why
				}
			}
		}
		m.enqueue(queued{id: id, item: item, sent: capSent(msg)})
	}
}

func (m *Model) listed(kind, id string) bool {
	for _, q := range m.queue {
		if q.item.Kind == kind && q.id == id {
			return true
		}
	}
	return false
}

// composeRunStarted is a run's start: the verb goes back to Steer, the
// items the last run settled go, and a follow_up or held prompt whose run
// this is has been delivered.
func (m *Model) composeRunStarted(id *string) {
	m.verb = view.VerbSteer
	kept := m.queue[:0]
	for _, q := range m.queue {
		if q.item.Fate == view.FateQueued {
			kept = append(kept, q)
		}
	}
	m.queue = kept
	if id != nil {
		m.settle(kindFollowUp, *id, view.FateDelivered, 0, "")
		m.settle(kindPrompt, *id, view.FateDelivered, 0, "")
	}
}

// composeState is what the view draws of the composer.
func (m *Model) composeState() view.Compose {
	c := view.Compose{Verb: m.verb, Cycles: m.composeTabs(), Next: nextVerb(m.verb, false), Holding: m.held != nil,
		TakeBack: m.Running && m.withdrawable() >= 0}
	for _, q := range m.queue {
		c.Queue = append(c.Queue, q.item)
	}
	return c
}
