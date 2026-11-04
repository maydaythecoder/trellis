package model

import (
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// tokenLine is rpc's model token event as the child writes it: at DEBUG, the
// one level rpc's stdout layer admits it at (#1059).
func tokenLine(text string) string {
	return jsonLine(map[string]any{"level": "DEBUG",
		"fields": map[string]any{"message": "model token", "text": text}})
}

// drawn is the transcript's rows as the view gets them, as text.
func drawn(m *Model) []string {
	var out []string
	for _, r := range m.viewState().Transcript {
		out = append(out, r.Text)
	}
	return out
}

// The risks (#1059, and the ratatui TUI's
// tokens_join_into_lines_rather_than_one_line_each): a colibri turn of
// minutes during which the screen shows nothing; a row per delta; model
// text with an escape sequence reaching the screen; deltas that stay on
// screen beside the turn's result, or that enter the transcript as if
// they were it. The deltas of the turn in progress are drawn joined into
// lines under a label saying what they are, made plain, and the turn's
// event, run_end or the child's exit drops them.
func TestStreamedTokensDrawnUntilTheTurnReplacesThem(t *testing.T) {
	m := running(t)
	m.Update(windowSize(120, 40))
	feed(m, event("model call", map[string]any{"turn": 1}))
	before := texts(m)
	for _, d := range []string{"I will ", "read\n", "a.py ", "\x1b[31mnow"} {
		feed(m, tokenLine(d))
	}
	if got := texts(m); strings.Join(got, "\n") != strings.Join(before, "\n") {
		t.Fatalf("the deltas entered the transcript itself: %q", got)
	}
	got := drawn(m)
	tail := got[len(before):]
	want := []string{streamLabel, "I will read", "a.py [31mnow"}
	if strings.Join(tail, "\n") != strings.Join(want, "\n") {
		t.Fatalf("streamed rows %q, want %q", tail, want)
	}
	for _, r := range m.viewState().Transcript[len(before):] {
		if r.Role != view.RoleText {
			t.Errorf("a streamed row of role %v, not the model's words: %q", r.Role, r.Text)
		}
	}
	screen := m.View()
	if !strings.Contains(screen, "a.py") || !strings.Contains(screen, "streamed; its words, not a verdict") {
		t.Fatalf("the reply in progress is not on screen:\n%s", screen)
	}
	if strings.Contains(screen, "\x1b[31mnow") {
		t.Fatalf("the model's escape sequence reached the screen")
	}

	feed(m, turn(1, "read_file a.py", map[string]any{"tool": "read_file", "target": "a.py"}))
	if d := drawn(m); strings.Join(d, "\n") != strings.Join(texts(m), "\n") {
		t.Fatalf("the deltas outlived their turn: %q", d)
	}
	if screen := m.View(); strings.Contains(screen, "streamed; its words") || !strings.Contains(screen, "Read a.py") {
		t.Fatalf("the turn did not replace the deltas:\n%s", screen)
	}

	// A run that ends, or a child that exits, mid-reply drops them too.
	feed(m, tokenLine("half a "))
	feed(m, runEnd(map[string]any{"outcome": "interrupted", "exit": 130}))
	if len(drawn(m)) != len(m.Transcript) {
		t.Fatalf("the deltas outlived run_end: %q", drawn(m))
	}
	submit(m, "again")
	feed(m, tokenLine("half a "))
	m.Update(ExitedMsg{})
	if len(drawn(m)) != len(m.Transcript) {
		t.Fatalf("the deltas outlived the child: %q", drawn(m))
	}
}

// The risk: a reply of minutes growing the model without bound. Only its
// newest StreamCap bytes are kept, cut at a character's start, and the
// label says older ones were dropped.
func TestStreamedReplyKeepsItsNewestBytes(t *testing.T) {
	m := running(t)
	for i := 0; i < StreamCap/4+10; i++ {
		feed(m, tokenLine("é é"))
	}
	if len(m.streamed) > StreamCap {
		t.Fatalf("kept %d bytes, past %d", len(m.streamed), StreamCap)
	}
	if !utf8.ValidString(m.streamed) {
		t.Fatalf("the cut split a character")
	}
	if d := drawn(m); !strings.HasSuffix(d[len(m.Transcript)], "; its last 16 KiB") {
		t.Fatalf("the label does not say the reply was cut: %q", d[len(m.Transcript)])
	}
}
