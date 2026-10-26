// Package themetest gives tests a Lip Gloss renderer at a fixed colour depth
// and background, so a test reads the same colours whatever terminal, or
// none, runs it. Build the theme from it with theme.New; this package does
// not import theme, so the theme's own tests can use it too.
package themetest

import (
	"io"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

// Depths are the colour depths a terminal reports, deepest first, by the
// names Lip Gloss's profiles give them.
var Depths = []string{"TrueColor", "ANSI256", "ANSI", "Ascii"}

// Renderer is a renderer at the named colour depth, on a dark or a light
// background. Lip Gloss takes the depth as a termenv profile, and this
// module does not import termenv (Lip Gloss's dependency, not its own: ADR
// 0052 section 3), so each depth is given as termenv's number for it; the
// name check makes a renumbering fail here rather than test the wrong depth.
func Renderer(t testing.TB, depth string, dark bool) *lipgloss.Renderer {
	t.Helper()
	r := lipgloss.NewRenderer(io.Discard)
	switch depth {
	case "TrueColor":
		r.SetColorProfile(0)
	case "ANSI256":
		r.SetColorProfile(1)
	case "ANSI":
		r.SetColorProfile(2)
	case "Ascii":
		r.SetColorProfile(3)
	default:
		t.Fatalf("no colour depth %q", depth)
	}
	if got := r.ColorProfile().Name(); got != depth {
		t.Fatalf("the profile set for %s is %s", depth, got)
	}
	r.SetHasDarkBackground(dark)
	return r
}
