// Package view draws the Go TUI. It holds no state and changes none: each
// function takes what it draws and returns the text, so a test reads the
// screen as a string. It imports nothing of the TUI's own, so the model can
// use it without a cycle.
package view

import (
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// ModalTitle is the approval modal's title, the ratatui TUI's.
const ModalTitle = "approve this change?  [y] accept  [n] reject"

// Modal draws the approval question in a bordered box at the centre of a
// width by height screen, and nothing else: what was under it is covered,
// not showing through, so the keys on screen are the ones that act.
func Modal(text string, width, height int) string {
	boxW := width * 6 / 10
	if boxW < len(ModalTitle)+4 {
		boxW = len(ModalTitle) + 4
	}
	if boxW > width {
		boxW = width
	}
	inner := boxW - 2
	if inner < 1 {
		inner = 1
	}
	var lines []string
	lines = append(lines, cut(ModalTitle, inner), strings.Repeat("─", inner))
	for _, l := range strings.Split(text, "\n") {
		lines = append(lines, wrap(l, inner)...)
	}
	maxLines := height - 2
	if maxLines < 1 {
		maxLines = 1
	}
	if len(lines) > maxLines {
		lines = append(lines[:maxLines-1], "…")
	}
	box := lipgloss.NewStyle().
		Border(lipgloss.NormalBorder()).
		Width(inner).
		Render(strings.Join(lines, "\n"))
	return lipgloss.Place(width, height, lipgloss.Center, lipgloss.Center, box)
}

// cut keeps at most w runes, so a long line is cut rather than wrapped.
func cut(s string, w int) string {
	r := []rune(s)
	if len(r) <= w {
		return s
	}
	if w <= 1 {
		return string(r[:w])
	}
	return string(r[:w-1]) + "…"
}

// wrap breaks a line into rows of at most w runes.
func wrap(s string, w int) []string {
	r := []rune(s)
	if len(r) == 0 {
		return []string{""}
	}
	var out []string
	for len(r) > w {
		out = append(out, string(r[:w]))
		r = r[w:]
	}
	return append(out, string(r))
}
