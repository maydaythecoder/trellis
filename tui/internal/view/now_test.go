package view

import (
	"strings"
	"testing"
)

// The risk (#857, the brand book's register dial, the desktop's #836): the
// playful register reads as a status, a face that differs by outcome
// passing for a verdict, or it changes a figure, a state word, a check, an
// outcome or a key, so the two registers tell the person different things.
// Every screen is drawn in both: playful differs only on the Now strip,
// and there only by one face and its copy, before any run and once one has
// ended, the same face for every outcome; never while a run is in flight,
// paused, stopping or waiting on the person, and never a loader. Serious
// draws no face anywhere.
func TestRegisterDialChangesCopyAndFacesOnly(t *testing.T) {
	ended := func(outcome string, exit int64) State {
		s := shellState()
		s.Running = false
		s.Now = Now{Outcome: outcome, Exit: &exit}
		s.Outcome = outcome
		return s
	}
	idle := State{Model: "ollama:qwen2.5-coder:32b"}
	paused := shellState()
	paused.Now.Paused = true
	waiting := shellState()
	waiting.Now.Waiting = true
	stopping := shellState()
	stopping.Now.Stopping = true
	selected := ended("converged", 0)
	selected.Nav.Selected = PanelSummary
	inside := ended("interrupted", 130)
	inside.Nav = Nav{Open: PanelChecks, Selected: PanelChecks}
	cases := []struct {
		name string
		s    State
		face string
	}{
		{"idle", idle, FaceEmpty + " nothing here yet"},
		{"running", shellState(), ""},
		{"paused", paused, ""},
		{"waiting", waiting, ""},
		{"stopping", stopping, ""},
		{"converged", ended("converged", 0), FaceEnded + " that one is over"},
		{"not converged", ended("not_converged", 1), FaceEnded + " that one is over"},
		{"error", ended("error", 2), FaceEnded + " that one is over"},
		{"summary selected", selected, FaceEnded + " that one is over"},
		{"inside a pane", inside, FaceEnded + " that one is over"},
	}
	for _, c := range cases {
		for _, size := range [][2]int{{80, 24}, {120, 40}} {
			serious := c.s
			playful := c.s
			playful.Playful = true
			sr := strings.Split(Render(serious, size[0], size[1]), "\n")
			pr := strings.Split(Render(playful, size[0], size[1]), "\n")
			if len(sr) != len(pr) {
				t.Fatalf("%s %v: %d rows serious, %d playful", c.name, size, len(sr), len(pr))
			}
			for _, face := range []string{FaceEmpty, FaceEnded} {
				if strings.Contains(strings.Join(sr, "\n"), face) {
					t.Fatalf("%s %v: serious draws a face:\n%s", c.name, size, strings.Join(sr, "\n"))
				}
			}
			for i := range sr {
				if i == 1 {
					continue
				}
				if sr[i] != pr[i] {
					t.Fatalf("%s %v: row %d differs off the Now strip:\nserious %q\nplayful %q", c.name, size, i, sr[i], pr[i])
				}
			}
			faces := strings.Count(pr[1], FaceEmpty) + strings.Count(pr[1], FaceEnded)
			if c.face == "" {
				if pr[1] != sr[1] {
					t.Fatalf("%s %v: a face with a run to show:\nserious %q\nplayful %q", c.name, size, sr[1], pr[1])
				}
				continue
			}
			if faces != 1 || !strings.Contains(pr[1], c.face) {
				t.Fatalf("%s %v: want one %q on the strip, got %q", c.name, size, c.face, pr[1])
			}
			// Without the face and its copy, the strip says exactly what the
			// serious one says: the same words, figures and keys, in order.
			rest := strings.Fields(strings.Replace(pr[1], c.face, "", 1))
			if strings.Join(rest, " ") != strings.Join(strings.Fields(sr[1]), " ") {
				t.Fatalf("%s %v: the strip's words changed:\nserious %q\nplayful %q", c.name, size, sr[1], pr[1])
			}
		}
	}
	// A strip with no room for the face gives the face up, never a word or
	// a key: at 100 columns a selected way into the summary is drawn whole.
	s := ended("not_converged", 1)
	s.Nav.Selected = PanelSummary
	s.Playful = true
	strip := strings.Split(Render(s, FoldWidth, FoldHeight), "\n")[1]
	if !strings.Contains(strip, "[summary]  [⏎] in") {
		t.Fatalf("the face pushed the way in off the strip: %q", strip)
	}
}
