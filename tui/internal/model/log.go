package model

import (
	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The action log's filter, quiet mode and raw call (#948), the desktop's
// #949 for the TUI. Each is a control key, since every printable key is
// the task line's, and none is one #857 took (^P ^R ^T ^E), ^S or ^Q
// (flow control), ^B (tmux's prefix) or ^Z (suspend): ^F the next filter,
// ^N quiet mode on or off, ^O the call of the turn the transcript draws
// last. None of them sends anything, and none takes a row out of the
// transcript: the view chooses which rows to draw (view/log.go).

// callOpen is the call box ^O opened: the turn's call and observation and
// the line it is scrolled to.
type callOpen struct {
	raw    *view.RawCall
	scroll int
}

// logKey takes ^F, ^N and ^O, and reports whether it took k.
func (m *Model) logKey(k tea.KeyMsg) bool {
	switch k.Type {
	case tea.KeyCtrlF:
		m.filter = nextFilter(m.filter)
	case tea.KeyCtrlN:
		m.quiet = !m.quiet
	case tea.KeyCtrlO:
		m.openCall()
		return true
	default:
		return false
	}
	// Fewer rows drawn is a lower top: an offset past it would spend
	// presses on rows not there (#843).
	m.ScrollBack = min(m.ScrollBack, m.transcriptTop())
	return true
}

// nextFilter is the filter after f in view.LogFilters, wrapping round.
func nextFilter(f view.LogFilter) view.LogFilter {
	for i, g := range view.LogFilters {
		if g == f {
			return view.LogFilters[(i+1)%len(view.LogFilters)]
		}
	}
	// The zero filter is All.
	return view.LogFilters[1]
}

// openCall is ^O: the call box for the newest turn whose rows the
// transcript draws now. Inside a pane the transcript is not drawn, so
// there is no turn on screen to open.
func (m *Model) openCall() {
	w, h := m.size()
	raw, ok := view.TranscriptCall(m.viewState(), w, h)
	if m.nav.Open != "" || !ok {
		m.push(view.RoleNote, "· no turn's call is in the transcript on screen; scroll to one, or change the filter")
		return
	}
	m.call = &callOpen{raw: raw}
}

// callKey is the call box's keys: ESC closes it, the arrows and the pages
// scroll it, Home and End go to either end. Nothing else acts under it.
func (m *Model) callKey(k tea.KeyMsg) {
	switch k.Type {
	case tea.KeyEsc:
		m.call = nil
	case tea.KeyUp:
		m.scrollCall(-1)
	case tea.KeyDown:
		m.scrollCall(1)
	case tea.KeyPgUp:
		m.scrollCall(-ModalPage)
	case tea.KeyPgDown:
		m.scrollCall(ModalPage)
	case tea.KeyHome:
		m.call.scroll = 0
	case tea.KeyEnd:
		w, h := m.size()
		m.call.scroll = view.CallLastFrom(*m.call.raw, w, h)
	}
}

// scrollCall moves the call box's first line by d, held between the first
// line and the box's last page, as the approval box is (#843).
func (m *Model) scrollCall(d int) {
	w, h := m.size()
	m.call.scroll = max(min(m.call.scroll+d, view.CallLastFrom(*m.call.raw, w, h)), 0)
}

// rawCall is a turn event's call and observation for ^O, each cut to
// RawCap (caps.go) and made plain by the view when drawn.
func rawCall(t *frames.TurnFields) *view.RawCall {
	r := &view.RawCall{Turn: int(t.Turn), Call: capRaw(t.Call)}
	if t.Observation != nil {
		o := capRaw(*t.Observation)
		r.Observation = &o
	}
	return r
}
