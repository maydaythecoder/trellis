package model

import (
	tea "github.com/charmbracelet/bubbletea"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// Levels (#851): the run view's top level draws the transcript and its
// panes; Tab selects a pane, ⏎ on an empty task line goes into it, and ESC
// comes back out, with the pane still selected. At the top ESC only clears
// the task line, and ^C is the only key that stops a run (owner decision,
// 2026-10-25). Each level draws what the model already holds, so going in
// or out sends nothing and starts nothing.

// navKey handles the keys that move between levels, and reports whether
// it took k. It is asked before the task line's own keys.
func (m *Model) navKey(k tea.KeyMsg) bool {
	switch k.Type {
	case tea.KeyEsc:
		switch {
		case m.nav.Open != "":
			m.nav.Open = ""
		case m.Input != "":
			m.Input = ""
			// The @ offer goes with the line it was for (#904).
			m.inputChanged()
		default:
			// At the top with an empty line ESC has nothing to leave: it
			// lets go of a selection, and never quits or stops a run.
			m.nav.Selected = ""
		}
		return true
	case tea.KeyEnter:
		if m.nav.Open == "" && m.Input == "" && m.nav.Selected != "" {
			m.nav.Open = m.nav.Selected
			return true
		}
	case tea.KeyTab, tea.KeyShiftTab:
		// An @ word on the task line keeps Tab for its completions.
		if _, completing := m.atFragment(); completing && k.Type == tea.KeyTab {
			return false
		}
		back := k.Type == tea.KeyShiftTab
		if m.nav.Open != "" {
			// Inside a pane Tab goes across to the next one, as the rail
			// beside it lists them.
			m.nav.Open = nextPanel(m.nav.Open, back)
			m.nav.Selected = m.nav.Open
		} else {
			m.nav.Selected = nextPanel(m.nav.Selected, back)
		}
		return true
	case tea.KeyUp, tea.KeyDown, tea.KeyPgUp, tea.KeyPgDown, tea.KeyHome, tea.KeyEnd:
		// A pane does not scroll yet; the transcript keeps its offset for
		// when the person comes back out.
		return m.nav.Open != ""
	}
	return false
}

// nextPanel is the pane after p in view.Panels, or before it going back,
// wrapping round; from none, the first or the last.
func nextPanel(p view.Panel, back bool) view.Panel {
	n := len(view.Panels)
	at := -1
	for i, q := range view.Panels {
		if q == p {
			at = i
		}
	}
	switch {
	case at < 0 && back:
		return view.Panels[n-1]
	case at < 0:
		return view.Panels[0]
	case back:
		return view.Panels[(at+n-1)%n]
	}
	return view.Panels[(at+1)%n]
}
