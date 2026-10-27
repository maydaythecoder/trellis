package view

import (
	"strings"
	"testing"

	"github.com/maydaythecoder/trellis/tui/internal/theme"
	"github.com/maydaythecoder/trellis/tui/internal/theme/themetest"
)

// #911: a Meter fills its cells to the figure's share of its ceiling,
// rounded down, but an empty Meter is exactly 0 and a full one is at its
// ceiling or past it; the filled cells are cool, signal past 70% and down
// past 90%, the empty ones line-strong, and nothing says a percentage. With
// a figure unreported there are no cells, only the figures, Dash for the
// one missing.
func TestMeterFillsToTheShareAndKeepsTheBudgetTones(t *testing.T) {
	for _, c := range []struct {
		used, ceiling uint64
		cells, want   int
	}{
		{0, 40, 8, 0}, {1, 40, 8, 1}, {5, 40, 8, 1}, {20, 40, 8, 4}, {39, 40, 8, 7},
		{40, 40, 8, 8}, {41, 40, 8, 8}, {0, 0, 8, 8}, {3, 6, 16, 8}, {39, 40, 16, 15},
	} {
		if got := meterFilled(c.used, c.ceiling, c.cells); got != c.want {
			t.Errorf("%d of %d in %d cells fills %d, want %d", c.used, c.ceiling, c.cells, got, c.want)
		}
	}
	for _, c := range []struct {
		used uint64
		want Role
	}{{0, RoleMeter}, {28, RoleMeter}, {29, RoleMeterNear}, {36, RoleMeterNear}, {37, RoleMeterOver}, {40, RoleMeterOver}, {44, RoleMeterOver}} {
		if got := meterRole(c.used, 40); got != c.want {
			t.Errorf("%d of 40 takes role %d, want %d", c.used, got, c.want)
		}
	}

	th := theme.New(themetest.Renderer(t, "TrueColor", true))
	out := line(th, meter("TURNS", u(29), u(40), meterCells), 60)
	for _, want := range []string{
		th.InkSecondary.Render("TURNS "), th.Signal.Render(strings.Repeat("█", 11)),
		// The space before the readout is ink too, so they are one piece.
		th.LineStrong.Render(strings.Repeat("░", 5)), th.Ink.Render(" 29/40"),
	} {
		if !strings.Contains(out, want) {
			t.Fatalf("%q is not in the painted Meter %q", want, out)
		}
	}
	if strings.Contains(out, "%") {
		t.Fatalf("a Meter said a percentage: %q", out)
	}
	for _, c := range []struct {
		used, ceiling *uint64
		want          string
	}{{nil, u(40), "TURNS —/40"}, {u(3), nil, "TURNS 3/—"}, {nil, nil, "TURNS —/—"}} {
		if got := spansText(meter("TURNS", c.used, c.ceiling, meterCells)); got != c.want {
			t.Errorf("got %q, want %q", got, c.want)
		}
	}
}

// #911: a row in more than one role is cut on its plain text, as Cut cuts
// one, and each piece is painted in its own role, the "…" in the role of
// the span it cut; it is exactly the width asked for. A row of one role
// draws as fill always has: Cut, then paint, then pad.
func TestLineCutsAcrossSpansAndPaintsEachPiece(t *testing.T) {
	th := theme.New(themetest.Renderer(t, "TrueColor", true))
	spans := []Span{{Text: "abc", Role: RoleYou}, {Text: "defgh", Role: RoleTool}}
	cut := line(th, spans, 6)
	if cut != th.Signal.Render("abc")+th.Cool.Render("de…") {
		t.Fatalf("cut row %q", cut)
	}
	whole := line(th, spans, 10)
	if whole != th.Signal.Render("abc")+th.Cool.Render("defgh")+"  " {
		t.Fatalf("whole row %q", whole)
	}
	for _, w := range []int{0, 1, 2, 6, 10} {
		if got := Width(unpainted(th, line(th, spans, w))); got != w {
			t.Fatalf("at %d cells the row is %d", w, got)
		}
	}
	for _, s := range []string{"", "short", strings.Repeat("应用", 30), "a b  c"} {
		for _, w := range []int{1, 5, 12, 80} {
			c := Cut(s, w)
			if got, want := fill(th, RoleNote, s, w), paint(th, RoleNote, c)+strings.Repeat(" ", max(w-Width(c), 0)); got != want {
				t.Fatalf("fill(%q, %d) = %q, want %q", s, w, got, want)
			}
		}
	}
}

// #911: a key is drawn as its keycap, in brackets with what it does after
// it, and a Status is a dot and its word, never a dot alone.
func TestKeycapsAndStatusDrawTheirCards(t *testing.T) {
	if got := keyRow(key{"TAB", "select"}, key{"⏎", "in"}, key{"^C", "stop"}); got != "[TAB] select  [⏎] in  [^C] stop" {
		t.Fatalf("keys %q", got)
	}
	if got := statusSpan("1 pending", RoleTool); got.Text != "● 1 pending" || got.Role != RoleTool {
		t.Fatalf("status %+v", got)
	}
}

// #911: the status bar's worktree is a Status in its tone: pending edits
// of a run in flight in the engine's cool, files left after it for the
// person's review in signal, and clean as a label; the keys are keycaps,
// ^C stopping a run in flight and leaving otherwise.
func TestStatusBarDrawsTheWorktreeAsAStatusAndTheKeysAsKeycaps(t *testing.T) {
	th := theme.New(themetest.Renderer(t, "TrueColor", true))
	lastRow := func(s State) string {
		s.Theme = th
		rows := strings.Split(Render(s, 120, 40), "\n")
		return rows[len(rows)-1]
	}
	s := shellState()
	if bar := lastRow(s); !strings.Contains(bar, th.Cool.Render("● 1 pending")) || !strings.Contains(bar, th.InkSecondary.Render(" [TAB] select  [⏎] in  [^F] filter  [^N] quiet  [^O] call  [^C] stop")) {
		t.Fatalf("running: %q", bar)
	}
	s.Running = false
	if bar := lastRow(s); !strings.Contains(bar, th.Signal.Render("● 1 to review")) || !strings.Contains(bar, "[^C] leave") {
		t.Fatalf("after the run: %q", bar)
	}
	// Clean is a label, painted as one piece with the model and tier beside
	// it, so its tone is read from worktree.
	s.Edits = nil
	if word, tone := worktree(s); word != "clean" || tone != RoleLabel || !strings.Contains(lastRow(s), "● clean  model") {
		t.Fatalf("clean: %q in role %d", word, tone)
	}
}
