package model

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func lines(m *Model) string { return strings.Join(texts(m), "\n") }

// /help lists the view's commands, /codebase and /graph among them, and
// starts no run.
func TestSlashHelpListsTheCommands(t *testing.T) {
	m := fresh(t)
	typeLine(m, "/help")
	enter(m)
	all := lines(m)
	for _, want := range []string{"> /help", "/model", "/graph", "/radius", "/codebase", "/clear", "@<text>", "esc"} {
		if !strings.Contains(all, want) {
			t.Fatalf("%q missing:\n%s", want, all)
		}
	}
	if out := m.TakeOutbox(); len(out) != 0 || m.Running {
		t.Fatalf("/help sent %+v", out)
	}
}

// /model asks get_state and says the model and tier it answers.
func TestSlashModelAsksGetState(t *testing.T) {
	m := fresh(t)
	typeLine(m, "/model")
	enter(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "get_state" || m.Running {
		t.Fatalf("%+v", out)
	}
	respond(m, out[0].Cmd["id"], "get_state", map[string]any{"model": "ollama:qwen", "tier": "1, native tool calls"})
	all := lines(m)
	if !strings.Contains(all, "· model  ollama:qwen") || !strings.Contains(all, "  tier   1, native tool calls") {
		t.Fatalf("%s", all)
	}
}

// /graph reports the last radius's three counts without a run, and never
// as a ratio; /radius <symbol> asks get_radius and shows reach and doubt as
// counts of their own.
func TestSlashGraphAndRadiusReportCountsNeverARatio(t *testing.T) {
	m := fresh(t)
	typeLine(m, "/graph")
	enter(m)
	if !strings.Contains(lines(m), "nothing yet") || len(m.TakeOutbox()) != 0 {
		t.Fatalf("%s", lines(m))
	}
	feed(m, turn(1, "blast_radius f", map[string]any{"resolved": 47, "candidates": 3, "unresolved": 2}))
	typeLine(m, "/graph")
	enter(m)
	if !strings.Contains(lines(m), "· reach: 47 resolved · 3 candidates · 2 unresolved") {
		t.Fatalf("%s", lines(m))
	}
	typeLine(m, "/radius app.core.f")
	enter(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "get_radius" || out[0].Cmd["symbol"] != "app.core.f" {
		t.Fatalf("%+v", out)
	}
	respond(m, out[0].Cmd["id"], "get_radius", map[string]any{"symbol": "app.core.f",
		"reach": map[string]any{"resolved": 12, "ambiguous": 4, "symbols_reached": 9},
		"doubt": map[string]any{"unbound_in_touched_files": 7, "touched_files": 3}})
	all := lines(m)
	if !strings.Contains(all, "app.core.f: reach 12 resolved · 4 ambiguous · 9 symbols reached") ||
		!strings.Contains(all, "doubt: 7 unbound call sites in 3 touched files") || strings.Contains(all, "%") {
		t.Fatalf("%s", all)
	}
}

// /codebase is handed to trellis as get_codebase, and its answer drawn
// with bound, ambiguous and unresolved as separate counts.
func TestSlashCodebaseAsksGetCodebase(t *testing.T) {
	m := fresh(t)
	typeLine(m, "/codebase")
	enter(m)
	out := m.TakeOutbox()
	if len(out) != 1 || out[0].Cmd["type"] != "get_codebase" {
		t.Fatalf("%+v", out)
	}
	respond(m, out[0].Cmd["id"], "get_codebase", map[string]any{
		"modules_indexed": 2, "bound_call_sites": 5, "ambiguous": 1, "unresolved": 2, "modules_not_drawn": 0,
		"modules": []any{map[string]any{"module": "app.core", "defines": 3,
			"calls": []any{map[string]any{"module": "app.util", "call_sites": 4}}}}})
	all := lines(m)
	if !strings.Contains(all, "codebase graph — 2 module(s)") || !strings.Contains(all, "5 bound call sites · 1 ambiguous · 2 unresolved") ||
		!strings.Contains(all, "app.core  defines 3 · calls app.util (4)") {
		t.Fatalf("%s", all)
	}
}

// An unknown /command is refused with the Rust TUI's reply, and nothing is
// sent.
func TestSlashUnknownIsRefused(t *testing.T) {
	m := fresh(t)
	typeLine(m, "/deploy now")
	enter(m)
	if last(m) != "· `/deploy` is not a command here. /help lists them." || len(m.TakeOutbox()) != 0 || m.Running {
		t.Fatalf("%s", lines(m))
	}
}

// A line starting `trellis ` gets the "this is not a shell" reply and
// nothing is sent: it does not become an agent run.
func TestSlashTrellisSubcommandIsNotAShell(t *testing.T) {
	m := fresh(t)
	typeLine(m, "trellis model")
	enter(m)
	all := lines(m)
	if !strings.Contains(all, "· `trellis model` is a shell command, and this is not a shell.") ||
		!strings.Contains(all, "use /model if this view has one") || len(m.TakeOutbox()) != 0 || m.Running {
		t.Fatalf("%s", all)
	}
}

// /clear empties the transcript and says so.
func TestSlashClearEmptiesTheTranscript(t *testing.T) {
	m := fresh(t)
	feed(m, turn(1, "old", nil))
	typeLine(m, "/clear")
	enter(m)
	if len(m.Transcript) != 1 || texts(m)[0] != "· transcript cleared" {
		t.Fatalf("%q", texts(m))
	}
}

// #855: /clear kept the scroll back of the transcript it cleared, so the
// pane pointed above an empty transcript and the line saying it was
// cleared was out of view. It returns to following.
func TestSlashClearReturnsToFollowing(t *testing.T) {
	m := numbered(t, 200)
	m.Update(keyType(tea.KeyHome))
	if m.ScrollBack == 0 {
		t.Fatal("home did not scroll back")
	}
	typeLine(m, "/clear")
	enter(m)
	if m.ScrollBack != 0 {
		t.Fatalf("scroll back %d after /clear, want 0: following", m.ScrollBack)
	}
	if v := m.View(); !strings.Contains(v, "transcript cleared") || strings.Contains(v, "scrolled back") {
		t.Fatalf("the cleared transcript is not followed:\n%s", v)
	}
}
