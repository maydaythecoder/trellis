package view

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// logState is shellState with a turn whose call wrote and a refusal after
// it, so quiet mode has a call to keep and Tools a refusal to draw.
func logState() State {
	s := shellState()
	call3 := &RawCall{Turn: 3, Call: `{"tool":"edit","path":"src/parser.py","old":"tokens[:-1]","new":"tokens"}`,
		Observation: strp("applied edit to src/parser.py\n@@ -40,1 +40,1 @@\n-    return tokens[:-1]\n+    return tokens\x1b[2J")}
	s.Transcript = append(s.Transcript,
		Row{Text: TurnDivider(3, u(5100), u(208), u(13100)), Role: RoleLabel, Turn: 3},
		Row{Text: "· Edited src/parser.py (+1 −1)", Role: RoleTool, Turn: 3, Wrote: true, Raw: call3},
		Row{Text: "    applied edit to src/parser.py", Role: RoleOutput, Turn: 3, Raw: call3},
		Row{Text: "! bash refused: the command names a path outside the repository", Role: RoleRefusal},
	)
	return s
}

// #948: the transcript under the Tools filter in quiet mode, and a turn's
// raw call over the run view, at the 80-column fold and beside the panes,
// match the files in testdata. Run with -update to write them again:
// go test ./internal/view/ -run TestLogGoldenRenders -update
func TestLogGoldenRenders(t *testing.T) {
	for _, c := range []struct {
		name string
		w, h int
		call bool
	}{
		{"tools-quiet-80x24", 80, 24, false},
		{"tools-quiet-120x40", 120, 40, false},
		{"call-80x24", 80, 24, true},
		{"call-120x40", 120, 40, true},
	} {
		s := logState()
		got := ""
		if c.call {
			raw, ok := TranscriptCall(s, c.w, c.h)
			if !ok {
				t.Fatalf("%s: no call on screen", c.name)
			}
			s.CallOpen = true
			got = CallView(s, *raw, 0, c.w, c.h) + "\n"
		} else {
			s.Filter, s.Quiet = FilterTools, true
			got = Render(s, c.w, c.h) + "\n"
		}
		if strings.Contains(got, "\x1b") {
			t.Fatalf("%s: an escape from the observation reached the screen", c.name)
		}
		path := filepath.Join("testdata", "log-"+c.name+".golden")
		if *update {
			if err := os.WriteFile(path, []byte(got), 0o644); err != nil {
				t.Fatal(err)
			}
			continue
		}
		want, err := os.ReadFile(path)
		if err != nil {
			t.Fatalf("%v (write it with -update)", err)
		}
		if got != string(want) {
			t.Fatalf("%s differs from %s (rewrite it with -update if the change is meant):\n%s", c.name, path, got)
		}
	}
}

// The risk (#948): ^O opening a turn the person cannot see, the newest
// turn in the transcript rather than the one on screen. The call is the
// newest the pane draws under its filter and scroll.
func TestTranscriptCallIsTheNewestTurnOnScreen(t *testing.T) {
	s := logState()
	if raw, ok := TranscriptCall(s, 120, 40); !ok || raw.Turn != 3 {
		t.Fatalf("following the end, the call is %+v", raw)
	}
	// Under Checks, turn 3 wrote no check row: turn 2's is the newest drawn.
	s.Filter = FilterChecks
	if raw, ok := TranscriptCall(s, 120, 40); !ok || raw.Turn != 2 {
		t.Fatalf("under Checks, the call is %+v", raw)
	}
	// Under You no row a turn's call wrote is drawn, only their dividers.
	s.Filter = FilterYou
	if raw, ok := TranscriptCall(s, 120, 40); ok {
		t.Fatalf("under You, a call %+v", raw)
	}
	if HasCall(s) {
		t.Fatal("the status bar would offer ^O with no call drawn")
	}
}
