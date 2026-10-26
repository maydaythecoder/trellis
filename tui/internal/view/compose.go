package view

import "fmt"

// The composer (#852), the desktop's #830 for the TUI: while a run is in
// flight the task box names the verb ⏎ applies to the line, Steer, Queue
// or Stop and send, and under the line it lists what was sent and is not
// yet settled, each with where it stands.

// Verb is what ⏎ does with the line while a run is in flight.
type Verb int

const (
	// VerbSteer sends the line as a steer for the run's next model
	// request. It is the verb every run starts and ends with, so Stop and
	// send is never the one ⏎ uses without the person choosing it.
	VerbSteer Verb = iota
	// VerbQueue sends the line as a follow_up, a task trellis runs after
	// this run ends.
	VerbQueue
	// VerbStop stops the run, which rolls back, and sends the line as a
	// task once the run has ended and trellis has answered the stop.
	VerbStop
)

// Verbs is every verb, in the order Tab visits them.
var Verbs = []Verb{VerbSteer, VerbQueue, VerbStop}

// Word is the verb's name, as the task box's title writes it.
func (v Verb) Word() string {
	switch v {
	case VerbQueue:
		return "queue"
	case VerbStop:
		return "stop and send"
	}
	return "steer"
}

// does is what ⏎ does with the line under the verb.
func (v Verb) does() string {
	switch v {
	case VerbQueue:
		return "run after this run ends"
	case VerbStop:
		return "stop this run (rolls back), then run this"
	}
	return "queue for the next turn"
}

// Fate is where a queued item stands. Only an event moves it: a steer
// leaves get_state's pending before its delivered event is written (#808),
// so an item missing from a later pending is not thereby delivered.
type Fate int

const (
	// FateQueued is sent and not yet settled.
	FateQueued Fate = iota
	// FateAfter is a steer carried by the model request after turn After.
	FateAfter
	// FateDelivered is a follow-up or a held prompt whose own run has
	// started: a new run, after no turn of the one before.
	FateDelivered
	// FateNot is never delivered, for Why.
	FateNot
	// FateWithdrawn is taken back: trellis answered the withdraw with
	// success, and will never deliver it (#857).
	FateWithdrawn
)

// Queued is one item of the composer's list: a steer or a follow_up that
// get_state listed as pending, or the prompt a Stop and send holds.
type Queued struct {
	// Kind is "steer", "follow_up" or "prompt".
	Kind string
	// Text is what was sent, as trellis listed it: served text.
	Text  string
	Fate  Fate
	After int
	Why   string
	// Withdrawing is set while a withdraw for it waits on trellis's
	// answer; it is still queued until that answer says otherwise.
	Withdrawing bool
}

// Compose is what the task box draws of the composer.
type Compose struct {
	// Verb is what ⏎ does while a run is in flight.
	Verb Verb
	// Cycles is set when Tab would change the verb now: a run is in flight
	// and the line holds text and no @ word.
	Cycles bool
	// Next is the verb Tab would change it to.
	Next Verb
	// Holding is set while a Stop and send's line waits for its run to end
	// and for trellis's answer to the stop.
	Holding bool
	// Queue is the list, oldest first.
	Queue []Queued
	// TakeBack is set while ^T would take an item back: a run is in
	// flight and a steer or follow-up of its is still queued (#857).
	TakeBack bool
}

// state is the item's state words, as the desktop's queue words them.
func (q Queued) state() (string, Role) {
	switch q.Fate {
	case FateAfter:
		return fmt.Sprintf("delivered after turn %d", q.After), RoleTool
	case FateDelivered:
		return "delivered", RoleTool
	case FateNot:
		if q.Why == "" {
			return "not delivered", RoleNote
		}
		return "not delivered (" + Plain(FirstLine(q.Why)) + ")", RoleNote
	case FateWithdrawn:
		return "withdrawn", RoleLabel
	}
	if q.Withdrawing {
		return "queued, taking back", RoleLabel
	}
	return "queued", RoleLabel
}

// row is the item as one row of the list: its state, then what it is and
// its text. Delivered is cool, the engine's own doing, and never ok: ok is
// a passed check's colour (DESIGN.md section 3.1).
func (q Queued) row() Row {
	word, role := q.state()
	return Row{Text: word + " · " + Plain(q.Kind) + ": " + Plain(q.Text), Role: role}
}

// composeTitle is the task box's title: what ⏎ does with the line now.
func composeTitle(s State) string {
	switch {
	case s.Raise != "":
		// The line is the new ceiling (#857).
		return "raise " + string(s.Raise) + ", in " + s.Raise.Unit() + "  " + keyRow(key{"⏎", "send"}, key{"ESC", "cancel"})
	case s.Plan.Edit != nil:
		e := s.Plan.Edit
		return fmt.Sprintf("plan step %d of %d  ", e.At+1, len(e.Lines)) + keyRow(key{"⏎", "send as a steer"}, key{"ESC", "cancel"})
	case s.Running:
		v := s.Compose.Verb
		return v.Word() + "  " + keyRow(key{"⏎", v.does()})
	case s.Compose.Holding:
		return "task  stopping: the held line runs once trellis answers the stop"
	}
	// As keycaps (#911); the status bar has the keys that move between
	// levels and ^C.
	return "task  " + keyRow(key{"⏎", "run"}, key{"ESC", "clear"}) + "  /help"
}

// composeBottom is the task box's bottom border: the verb Tab changes to,
// when it would, and ^T while an item can be taken back (#857), which
// takes back the newest one still queued.
func composeBottom(s State) string {
	if !s.Running || s.Raise != "" || s.Plan.Edit != nil {
		return ""
	}
	var keys []key
	if s.Compose.Cycles {
		keys = append(keys, key{"TAB", s.Compose.Next.Word()})
	}
	if s.Compose.TakeBack {
		keys = append(keys, key{"^T", "take back the newest queued"})
	}
	return keyRow(keys...)
}

// queueRows is the list in at most room rows: oldest first, and when they
// do not all fit, the last row says how many are not shown.
func queueRows(q []Queued, room int) []Row {
	if room <= 0 || len(q) == 0 {
		return nil
	}
	var out []Row
	shown := q
	if len(q) > room {
		shown = q[:room-1]
	}
	for _, it := range shown {
		out = append(out, it.row())
	}
	if len(shown) < len(q) {
		out = append(out, Row{Text: fmt.Sprintf("… %d more queued", len(q)-len(shown)), Role: RoleLabel})
	}
	return out
}
