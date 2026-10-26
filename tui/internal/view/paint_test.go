package view

import (
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"

	"github.com/maydaythecoder/trellis/tui/internal/theme"
	"github.com/maydaythecoder/trellis/tui/internal/theme/themetest"
)

// unpainted is out with every escape sequence th paints with taken out:
// what each role's style puts before and after the text it draws.
func unpainted(th theme.Theme, out string) string {
	for r := Role(0); r < roles; r++ {
		before, after, _ := strings.Cut(style(th, r).Render("x"), "x")
		for _, seq := range []string{before, after} {
			if seq != "" {
				out = strings.ReplaceAll(out, seq, "")
			}
		}
	}
	return out
}

// Each role is painted in the token DESIGN.md section 3.1 gives its
// meaning: the person in signal, the model's words in ink, tools in cool,
// checks in ok and down, notes in warn, labels in ink-secondary and frames
// in line. A role left out of the switch would fall to ink and look like
// the model's words without a word said.
func TestPaintEachRoleTakesItsToken(t *testing.T) {
	th := theme.New(themetest.Renderer(t, "TrueColor", true))
	want := map[Role]lipgloss.Style{
		RoleText:    th.Ink,
		RoleYou:     th.Signal,
		RoleTool:    th.Cool,
		RolePassed:  th.OK,
		RoleFailed:  th.Down,
		RoleNote:    th.Warn,
		RoleLabel:   th.InkSecondary,
		RoleFrame:   th.Line,
		RoleOutput:  th.InkSecondary,
		RoleRefusal: th.Down,
		// A Meter's cells (#911): cool, signal past 70% of the ceiling, down
		// past 90%, and line-strong where empty.
		RoleMeter:      th.Cool,
		RoleMeterNear:  th.Signal,
		RoleMeterOver:  th.Down,
		RoleMeterEmpty: th.LineStrong,
		// A diff's lines (#853): the Diff card's three tokens.
		RoleDiffAdd:  th.DiffAdd,
		RoleDiffDel:  th.DiffDel,
		RoleDiffHunk: th.DiffHunk,
	}
	if len(want) != int(roles) {
		t.Fatalf("%d roles and %d named here: a new role needs its token named", roles, len(want))
	}
	for r, s := range want {
		if got := paint(th, r, "x"); got != s.Render("x") {
			t.Errorf("role %d drew %q, want %q", r, got, s.Render("x"))
		}
	}
	// Blank text is left alone: a colour on spaces shows nothing.
	if got := paint(th, RoleYou, "   "); got != "   " {
		t.Errorf("blank text drew %q", got)
	}
}

// A check row is ok or down from the verdict trellis sent, and only from
// it: the same words in the transcript, which the model could write, are
// drawn as text. So the colour of a passed check is never the model's to
// give (ADR 0004).
func TestViewCheckRowsTakeOkAndDownRoles(t *testing.T) {
	for _, depth := range []string{"TrueColor", "ANSI256", "ANSI"} {
		for _, dark := range []bool{true, false} {
			th := theme.New(themetest.Renderer(t, depth, dark))
			s := populated()
			s.Theme = th
			s.Checks = []Check{{Name: "typecheck", Passed: true}, {Name: "tests", Passed: false}}
			s.Transcript = rowsOf(RoleText, "typecheck passed", "tests failed")
			out := Render(s, 120, 40)
			for _, c := range []struct {
				row         string
				style, text lipgloss.Style
			}{
				{"typecheck passed", th.OK, th.Ink},
				{"tests failed", th.Down, th.Ink},
			} {
				if n := strings.Count(out, c.style.Render(c.row)); n != 1 {
					t.Errorf("%s, dark %v: %q is painted as a check %d times, want once (the checks pane):\n%q", depth, dark, c.row, n, out)
				}
				if !strings.Contains(out, c.text.Render(c.row)) {
					t.Errorf("%s, dark %v: the transcript's %q is not drawn as text:\n%q", depth, dark, c.row, out)
				}
			}
		}
	}
}
