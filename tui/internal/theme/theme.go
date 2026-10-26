// Package theme is the TUI's colours: every style it paints with, built once
// from one Lip Gloss renderer (New) out of the table in tokens_gen.go.
//
// tokens_gen.go is generated from docs/design/system/tokens.json by
// scripts/checks/design-tokens.py --write, and that check fails when the
// file is stale (DESIGN.md section 3.4). Each colour gives its value at
// every depth explicitly, as a CompleteAdaptiveColor, because Lip Gloss's
// own downgrading picks colours the brand book does not: at 16 colours line
// would become black, and play the same yellow as signal.
//
// A light terminal takes the light hex at truecolor; below truecolor it
// draws text in its own default foreground and every other role in its
// 16-colour index (owner decision, 2026-10-25). The soft fills and
// ink-inverse have no terminal token and are not in the theme yet: the
// change that first paints one adds it through the generator, painted at
// truecolor only, with ink-inverse drawn as reverse video below truecolor.
package theme

import "github.com/charmbracelet/lipgloss"

// Theme is every style the TUI paints with. Each sets a foreground colour,
// except the three surfaces, which set a fill. Build it once, with New.
type Theme struct {
	// Ground, Panel and PanelRaised are fills: the page, a panel, and a
	// hovered or selected row. Where the terminal has no colour for
	// PanelRaised (16 colours, or a light terminal below truecolor) it is
	// reverse video instead, as term-panel-raised's usage says.
	Ground, Panel, PanelRaised lipgloss.Style

	// Line and LineStrong draw frames and rules; Edge is a connector at
	// rest and EdgeActive the one being followed.
	Line, LineStrong, Edge, EdgeActive lipgloss.Style

	// Ink, InkSecondary and InkMuted are text, in three weights of
	// attention.
	Ink, InkSecondary, InkMuted lipgloss.Style

	// Each accent means one thing (DESIGN.md section 3.1): Signal the
	// person and what needs them, Cool the engine's own activity, Play the
	// playful register, OK a check that passed, Down one that failed or a
	// refusal, Warn waiting, Up scroll and direction hints.
	Signal, Cool, Play, OK, Down, Warn, Up lipgloss.Style

	// DiffAdd, DiffDel and DiffHunk are added and removed lines and hunk
	// headers.
	DiffAdd, DiffDel, DiffHunk lipgloss.Style
}

// New builds every style from r, which carries the colour depth and the
// background the colours are chosen for. Read both from the terminal once,
// before the Bubble Tea program starts (cmd/trellis-tui), and leave them:
// a style resolves its colour through r each time it renders.
func New(r *lipgloss.Renderer) Theme {
	fg := func(c lipgloss.CompleteAdaptiveColor) lipgloss.Style { return r.NewStyle().Foreground(c) }
	fill := func(c lipgloss.CompleteAdaptiveColor) lipgloss.Style { return r.NewStyle().Background(c) }
	t := Theme{
		Ground:       fill(termGround),
		Panel:        fill(termPanel),
		PanelRaised:  fill(termPanelRaised),
		Line:         fg(termLine),
		LineStrong:   fg(termLineStrong),
		Edge:         fg(termEdge),
		Ink:          fg(termInk),
		InkSecondary: fg(termInkSecondary),
		InkMuted:     fg(termInkMuted),
		Signal:       fg(termSignal),
		Cool:         fg(termCool),
		Play:         fg(termPlay),
		OK:           fg(termOk),
		Down:         fg(termDown),
	}
	// Roles that share a colour share an entry, as tokens.json gives them
	// one: warn and edge-active are signal, up and diff-hunk are cool,
	// diff-add is ok and diff-del is down.
	t.Warn, t.EdgeActive = t.Signal, t.Signal
	t.Up, t.DiffHunk = t.Cool, t.Cool
	t.DiffAdd, t.DiffDel = t.OK, t.Down
	if !paints(r, termPanelRaised) {
		t.PanelRaised = r.NewStyle().Reverse(true)
	}
	return t
}

// paints reports whether c has a colour at r's depth on r's background.
func paints(r *lipgloss.Renderer, c lipgloss.CompleteAdaptiveColor) bool {
	at := c.Light
	if r.HasDarkBackground() {
		at = c.Dark
	}
	// The depth is read by its name: the profile is a termenv type, and
	// this module does not import termenv, Lip Gloss's dependency rather
	// than its own (ADR 0052 section 3).
	switch r.ColorProfile().Name() {
	case "TrueColor":
		return at.TrueColor != ""
	case "ANSI256":
		return at.ANSI256 != ""
	case "ANSI":
		return at.ANSI != ""
	}
	return false
}
