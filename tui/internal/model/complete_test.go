package model

import (
	"fmt"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func tab(m *Model) { m.Update(tea.KeyMsg{Type: tea.KeyTab}) }

// Tab after @prefix sends list_paths {prefix}; one path completes the word
// with a space, several are listed in the order trellis returned them, and
// a second Tab inserts the first.
func TestCompleteOneChoiceCompletesSeveralAreListed(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix @src/mo")
	tab(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "list_paths" || out[0].Cmd["prefix"] != "src/mo" {
		t.Fatalf("sent %+v", out)
	}
	respond(m, out[0].Cmd["id"], "list_paths", map[string]any{"paths": []any{"src/models.py"}})
	if m.Input != "fix src/models.py " {
		t.Fatalf("input %q", m.Input)
	}
	typeLine(m, "@app")
	tab(m)
	out = m.TakeOutbox()
	respond(m, out[0].Cmd["id"], "list_paths", map[string]any{"paths": []any{"app/b.py", "app/a.py", "app/core/c.py"}})
	if strings.Join(m.Offered, ",") != "app/b.py,app/a.py,app/core/c.py" || m.Input != "fix src/models.py @app" {
		t.Fatalf("offered %q input %q", m.Offered, m.Input)
	}
	tab(m)
	if m.Input != "fix src/models.py app/b.py " || len(m.Offered) != 0 || len(m.TakeOutbox()) != 0 {
		t.Fatalf("second tab: input %q offered %q", m.Input, m.Offered)
	}
}

// Nothing offered changes nothing; an answer for a fragment no longer
// typed is dropped; only an @word at the very end is completed; a run in
// flight is not asked for paths.
func TestCompleteNothingOfferedOrStaleChangesNothing(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix @zz")
	tab(m)
	id := m.TakeOutbox()[0].Cmd["id"]
	respond(m, id, "list_paths", map[string]any{"paths": []any{}})
	if m.Input != "fix @zz" || len(m.Offered) != 0 {
		t.Fatalf("input %q offered %q", m.Input, m.Offered)
	}
	tab(m)
	id = m.TakeOutbox()[0].Cmd["id"]
	typeLine(m, "q")
	respond(m, id, "list_paths", map[string]any{"paths": []any{"zz.py"}})
	if m.Input != "fix @zzq" {
		t.Fatalf("a stale answer completed: %q", m.Input)
	}
	for _, line := range []string{"fix @a.py ", "fix a", "@"[:0]} {
		m.Input = line
		tab(m)
		if out := m.TakeOutbox(); len(out) != 0 {
			t.Fatalf("%q asked for paths: %+v", line, out)
		}
	}
	r := running(t)
	typeLine(r, "@src")
	tab(r)
	if out := r.TakeOutbox(); len(out) != 0 {
		t.Fatalf("a run in flight was asked for paths: %+v", out)
	}
}

// Candidates are drawn under the input line, the first marked as the one
// Tab inserts, and are not drawn while a run holds the input.
func TestCompleteCandidatesAreDrawnUnderTheInput(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix @models")
	tab(m)
	respond(m, m.TakeOutbox()[0].Cmd["id"], "list_paths", map[string]any{"paths": []any{"app/models.py", "src/models.py"}})
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	rows := strings.Split(m.View(), "\n")
	at := func(s string) int {
		for i, r := range rows {
			if strings.Contains(r, s) {
				return i
			}
		}
		t.Fatalf("%q not on screen:\n%s", s, strings.Join(rows, "\n"))
		return -1
	}
	typed, first, second := at("fix @models"), at("app/models.py"), at("src/models.py")
	if !(typed < first && first < second) || !strings.Contains(rows[first], "tab") || at("budget") < 0 || len(rows) != 24 {
		t.Fatalf("%s", strings.Join(rows, "\n"))
	}
}

// Enter with an @word sends the line as typed.
func TestCompleteEnterSubmitsTheLineAsTyped(t *testing.T) {
	m := fresh(t)
	submit(m, "fix @src/mo")
	out := m.TakeOutbox()
	if len(out) != 1 || out[len(out)-1].Cmd["task"] != "fix @src/mo" {
		t.Fatalf("%+v", out)
	}
}

// The client never builds an exec.Cmd: no file of this package imports
// os/exec, so completion can only come from trellis (ADR 0023).
func TestCompleteNeverBuildsAnExecCmd(t *testing.T) {
	files, err := filepath.Glob("*.go")
	if err != nil || len(files) == 0 {
		t.Fatalf("no sources: %v", err)
	}
	// Built at run time, so this test's own text is not the import
	// go-guards.sh refuses outside internal/rpc.
	banned := map[string]bool{`"os/` + `exec"`: true, `"sys` + `call"`: true}
	fset := token.NewFileSet()
	for _, f := range files {
		src, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		af, err := parser.ParseFile(fset, f, src, parser.ImportsOnly)
		if err != nil {
			t.Fatal(err)
		}
		for _, imp := range af.Imports {
			if banned[imp.Path.Value] {
				t.Fatalf("%s imports %s", f, imp.Path.Value)
			}
		}
		if strings.Contains(string(src), "exec.Command") && !strings.HasSuffix(f, "_test.go") {
			t.Fatalf("%s names exec.Command", f)
		}
	}
}

// A fragment ending in a letter of more than one byte completes. In UTF-8
// "à" is C3 A0 and "ą" is C4 85; read by its last byte, the line ended in
// U+00A0 or U+0085, both spaces to unicode.IsSpace, so the fragment read
// as finished and Tab asked trellis for nothing.
func TestCompleteFragmentEndingInAnAccentedLetter(t *testing.T) {
	for _, frag := range []string{"à", "ą"} {
		m := fresh(t)
		typeLine(m, "@"+frag)
		tab(m)
		out := m.TakeOutbox()
		if len(out) != 1 || out[0].Cmd["type"] != "list_paths" || out[0].Cmd["prefix"] != frag {
			t.Fatalf("@%s sent %+v", frag, out)
		}
		respond(m, out[0].Cmd["id"], "list_paths", map[string]any{"paths": []any{frag + ".py"}})
		if m.Input != frag+".py " {
			t.Fatalf("@%s completed to %q", frag, m.Input)
		}
	}
}

// An answer longer than OfferedCap is cut to its first OfferedCap paths,
// in the order returned, and a last line says how many more there were;
// Tab still inserts the first. The risk: each listed path is a row the
// entry box takes from the panes above it, and a 40-path answer drawn
// uncut at 80x24 left the transcript pane no room and View panicked. An
// answer of exactly OfferedCap claims no more.
func TestCompleteListIsCapped(t *testing.T) {
	answer := func(n int) *Model {
		m := fresh(t)
		typeLine(m, "fix @src/")
		tab(m)
		var paths []any
		for i := 1; i <= n; i++ {
			paths = append(paths, fmt.Sprintf("src/m%02d.py", i))
		}
		respond(m, m.TakeOutbox()[0].Cmd["id"], "list_paths", map[string]any{"paths": paths})
		return m
	}
	last := fmt.Sprintf("src/m%02d.py", OfferedCap)
	if m := answer(OfferedCap); len(m.Offered) != OfferedCap || m.Offered[OfferedCap-1] != last {
		t.Fatalf("exactly the cap: offered %q", m.Offered)
	}
	m := answer(OfferedCap + 30)
	if len(m.Offered) != OfferedCap+1 || m.Offered[0] != "src/m01.py" || m.Offered[OfferedCap-1] != last ||
		m.Offered[OfferedCap] != "30 more path(s) not listed" {
		t.Fatalf("offered %q", m.Offered)
	}
	// At 80x26 the whole capped list fits: at 80x24 the Now strip's row
	// (#850) and the status bar's under the task box (#851) leave it two
	// rows short, and the list then says what it cuts.
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 26})
	screen := m.View()
	if len(strings.Split(screen, "\n")) != 26 || !strings.Contains(screen, last) ||
		!strings.Contains(screen, "30 more path(s) not listed") || strings.Contains(screen, fmt.Sprintf("src/m%02d.py", OfferedCap+1)) {
		t.Fatalf("%s", screen)
	}
	tab(m)
	if m.Input != "fix src/m01.py " || len(m.Offered) != 0 {
		t.Fatalf("tab: input %q offered %q", m.Input, m.Offered)
	}
}

// #904: Esc empties the line and the @ offer made for it, so the task box
// lists no path for a fragment that is gone.
func TestCompleteEscClearsTheOffer(t *testing.T) {
	m := fresh(t)
	typeLine(m, "fix @models")
	tab(m)
	respond(m, m.TakeOutbox()[0].Cmd["id"], "list_paths", map[string]any{"paths": []any{"app/models.py", "src/models.py"}})
	m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	if !strings.Contains(m.View(), "app/models.py") {
		t.Fatalf("the offer is not drawn before Esc:\n%s", m.View())
	}
	m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	if m.Input != "" || len(m.Offered) != 0 || strings.Contains(m.View(), "app/models.py") {
		t.Fatalf("after Esc: input %q, offered %v\n%s", m.Input, m.Offered, m.View())
	}
	if m.Quitting {
		t.Fatal("the first Esc, on a line with text, left the program")
	}
}
