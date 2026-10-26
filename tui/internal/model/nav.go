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
		case m.nav.Diff:
			// Out of a file's diff to the code level, the cursor still on
			// the file.
			m.nav.Diff, m.nav.Scroll = false, 0
		case m.nav.Open != "":
			m.nav.Open, m.nav.Scroll = "", 0
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
			m.nav.Open, m.nav.Scroll = m.nav.Selected, 0
			return true
		}
		// At the code level ⏎ on an empty line goes into the cursor's
		// file's diff (#853), and with no file there is nothing to go into.
		if m.nav.Open == view.PanelEdits && !m.nav.Diff && m.Input == "" {
			if paths := view.CodePaths(m.viewState()); len(paths) > 0 {
				m.nav.File = paths[m.fileAt(paths)]
				m.nav.Diff, m.nav.Scroll = true, 0
			}
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
			// beside it lists them, from a file's diff as well.
			m.nav.Diff, m.nav.Scroll = false, 0
			m.nav.Open = nextPanel(m.nav.Open, back)
			m.nav.Selected = m.nav.Open
		} else {
			m.nav.Selected = nextPanel(m.nav.Selected, back)
		}
		return true
	case tea.KeyUp, tea.KeyDown, tea.KeyPgUp, tea.KeyPgDown, tea.KeyHome, tea.KeyEnd:
		// A file's diff and the summary level scroll (#853, #854), and the
		// code level's cursor moves; any other pane does not scroll yet. The
		// transcript keeps its offset for when the person comes back out.
		switch {
		case m.nav.Diff, m.nav.Open == view.PanelSummary, m.nav.Open == view.PanelPlan:
			m.scrollLevelKey(k.Type)
		case m.nav.Open == view.PanelEdits && (k.Type == tea.KeyUp || k.Type == tea.KeyDown):
			if paths := view.CodePaths(m.viewState()); len(paths) > 0 {
				d := 1
				if k.Type == tea.KeyUp {
					d = -1
				}
				m.nav.File = paths[max(min(m.fileAt(paths)+d, len(paths)-1), 0)]
			}
		}
		return m.nav.Open != ""
	}
	return false
}

// fileAt is the code level's cursor in paths: the file nav names, or the
// first when it names none of them.
func (m *Model) fileAt(paths []string) int {
	for i, p := range paths {
		if p == m.nav.File {
			return i
		}
	}
	return 0
}

// scrollLevelKey scrolls the diff level or the summary level by the key: a
// row for an arrow, a page for PgUp and PgDn, and to either end for Home
// and End.
func (m *Model) scrollLevelKey(k tea.KeyType) {
	switch k {
	case tea.KeyUp:
		m.scrollLevel(-1)
	case tea.KeyDown:
		m.scrollLevel(1)
	case tea.KeyPgUp:
		m.scrollLevel(-view.TranscriptPage)
	case tea.KeyPgDown:
		m.scrollLevel(view.TranscriptPage)
	case tea.KeyHome:
		m.nav.Scroll = 0
	case tea.KeyEnd:
		m.nav.Scroll = m.levelTop()
	}
}

// scrollLevel moves the level's first row by d, held between the first row
// and the view's own last page (levelTop), as the transcript and the
// approval box are held (#843).
func (m *Model) scrollLevel(d int) {
	m.nav.Scroll = max(min(m.nav.Scroll+d, m.levelTop()), 0)
}

// levelTop is the furthest the level shown scrolls: the summary's, or a
// file's diff's.
func (m *Model) levelTop() int {
	w, h := m.size()
	switch m.nav.Open {
	case view.PanelSummary:
		return view.SummaryTop(m.viewState(), w, h)
	case view.PanelPlan:
		return view.PlanTop(m.viewState(), w, h)
	}
	return view.CodeTop(m.viewState(), w, h)
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
