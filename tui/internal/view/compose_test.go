package view

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// composeState is the run view mid-run with Stop and send chosen for a
// typed line and one queue item in each state.
func composeState() State {
	s := shellState()
	s.Input = "start over from the parser"
	s.Compose = Compose{
		Verb:   VerbStop,
		Cycles: true,
		Next:   VerbSteer,
		Queue: []Queued{
			{Kind: "steer", Text: "use the helper", Fate: FateAfter, After: 2},
			{Kind: "steer", Text: "and keep the old name", Fate: FateNot, Why: "the run was stopped first"},
			{Kind: "follow_up", Text: "then rename parse_expr", Fate: FateQueued},
			{Kind: "prompt", Text: "start over\x1b[2J from the parser", Fate: FateDelivered},
		},
	}
	return s
}

// The risk (#852, #851): the composer's verb and queue pushing the shell
// past the screen at 80 columns, where the right column folds, or served
// text reaching it with its escape bytes. Every row is the screen's width,
// the title names the verb and the bottom border the next, and each item
// shows its state; the renders match the goldens in testdata.
func TestComposeGoldenRenders(t *testing.T) {
	for _, size := range [][2]int{{80, 24}, {120, 40}} {
		w, h := size[0], size[1]
		got := Render(composeState(), w, h)
		rows := strings.Split(got, "\n")
		if len(rows) != h {
			t.Fatalf("%dx%d: %d rows", w, h, len(rows))
		}
		for i, r := range rows {
			if Width(r) != w {
				t.Fatalf("%dx%d: row %d is %d cells:\n%s", w, h, i, Width(r), got)
			}
		}
		if strings.Contains(got, "\x1b") {
			t.Fatalf("%dx%d: an escape byte reached the screen", w, h)
		}
		for _, want := range []string{"stop and send  [⏎] stop this run", "[TAB] steer", "delivered after turn 2 · steer: use the helper", "not delivered (the run was stopped first)", "queued · follow_up:", "delivered · prompt:"} {
			if !strings.Contains(got, want) {
				t.Fatalf("%dx%d: no %q in:\n%s", w, h, want, got)
			}
		}
		path := filepath.Join("testdata", fmt.Sprintf("compose-%dx%d.golden", w, h))
		if *update {
			if err := os.WriteFile(path, []byte(got+"\n"), 0o644); err != nil {
				t.Fatal(err)
			}
			continue
		}
		want, err := os.ReadFile(path)
		if err != nil {
			t.Fatalf("%v (write it with -update)", err)
		}
		if got+"\n" != string(want) {
			t.Fatalf("%dx%d differs from %s (rewrite it with -update if the change is meant):\n%s", w, h, path, got)
		}
	}
}

// The risk: a queue longer than the room left drawn past the task box's
// share of the screen, or cut with no word that more are waiting. At
// 80x24 the list keeps the stage its floor and says how many it left out.
func TestComposeQueueIsCutAndSaysHowManyMore(t *testing.T) {
	s := composeState()
	for range 20 {
		s.Compose.Queue = append(s.Compose.Queue, Queued{Kind: "steer", Text: "more"})
	}
	got := Render(s, 80, 24)
	if rows := strings.Split(got, "\n"); len(rows) != 24 {
		t.Fatalf("%d rows", len(rows))
	}
	if !strings.Contains(got, " more queued") {
		t.Fatalf("the cut list does not say how many more:\n%s", got)
	}
}
