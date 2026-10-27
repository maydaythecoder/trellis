package model

import (
	"strings"
	"testing"

	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// The risks (#954): a run whose older observations were elided to fit its
// context budget looks, from the TUI, like one that saw everything; the
// request's size and the budget are divided into a share trellis never
// sent (CLAUDE.md rule 3's reasoning); and a figure no compacted event has
// said yet is drawn as 0. Where it is counts both kinds of compaction and
// gives the latest compacted request's bytes beside its budget, and the
// next run starts with none, as the desktop's
// where_counts_compactions_from_compacted_events holds it.
func TestWhereItIsCountsCompactions(t *testing.T) {
	m := running(t)
	m.Update(windowSize(120, 40))
	m.nav.Open = view.PanelWhere
	if v := m.View(); !strings.Contains(v, "context  0 compactions") || strings.Contains(v, "request ") {
		t.Fatalf("before any compaction:\n%s", v)
	}
	feed(m, event("compacted", map[string]any{"turn": 3, "elided_turns": 1, "elided_bytes": 400,
		"request_bytes": 900, "budget": 1000}))
	feed(m, event("compacted", map[string]any{"turn": 4, "elided_turns": 2, "elided_bytes": 700,
		"request_bytes": 950, "budget": 1000}))
	// A compact a client asked for is a compaction too.
	feed(m, event("compacted on request", map[string]any{"turn": 5, "turns_folded": 1, "elided_turns": 2,
		"bytes_before": 990, "bytes_after": 600}))
	v := m.View()
	if !strings.Contains(v, "context  3 compactions · request 950 B · budget 1000 B") {
		t.Fatalf("three compactions and the latest request's two figures:\n%s", v)
	}
	if strings.Contains(v, "%") {
		t.Fatalf("Where it is derives a share:\n%s", v)
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	feed(m, runStart("next"))
	if v := m.View(); !strings.Contains(v, "context  0 compactions") || strings.Contains(v, "request 950") {
		t.Fatalf("the next run kept the last one's compactions:\n%s", v)
	}
}

// The risk (#954): a radius whose observation folded call sites into
// per-file counts reads as a whole list, so the person thinks they saw
// every site. The row says how many, in the desktop's words, and a radius
// that listed every one says nothing more.
func TestRadiusTurnSaysHowManyCallSitesWereSummarised(t *testing.T) {
	m := running(t)
	radius := func(n int) string {
		return event("turn", map[string]any{"turn": 1, "call": "blast_radius f", "verdict": "None",
			"check_kind": "", "observation": "", "tool": "blast_radius", "target": "f", "wrote": false,
			"resolved": 40, "candidates": 2, "unresolved": 1, "summarised": n})
	}
	feed(m, radius(12))
	feed(m, radius(0))
	var said []string
	for _, l := range texts(m) {
		if strings.Contains(l, "Measured") {
			said = append(said, l)
		}
	}
	want := []string{"· Measured the blast radius of f; 12 call site(s) summarised by file", "· Measured the blast radius of f"}
	if strings.Join(said, "\n") != strings.Join(want, "\n") {
		t.Fatalf("radius rows %q, want %q", said, want)
	}
}

// The risk (#954): with a follow-up queued behind a prompt, a question is
// answered for the wrong request, since the box did not say which one's
// run asks. The row above the box names it from request_id, in the
// desktop's words, and says when it was sent with no id.
func TestApprovalNamesThePromptOrFollowUpItBelongsTo(t *testing.T) {
	ask := func(id string, requestID any) string {
		return jsonLine(map[string]any{"type": "approval_request", "id": id, "question": "edit a.py",
			"detail": "", "request_id": requestID})
	}
	m := running(t)
	m.Update(windowSize(120, 40))
	feed(m, jsonLine(map[string]any{"type": "run_start", "id": m.promptID, "task": "change f\x1b[31m"}))
	feed(m, ask("q1", m.promptID))
	if m.Modal == nil || m.Modal.For != "the prompt: change f[31m" {
		t.Fatalf("modal %+v", m.Modal)
	}
	if v := m.View(); !strings.Contains(v, " for the prompt: change f[31m") {
		t.Fatalf("the row above the box:\n%s", v)
	}
	key(m, 'y')

	// A follow-up's own run: its start carries its id.
	typeLine(m, "then rename it")
	tabKey(m)
	enter(m)
	out := m.TakeOutbox()
	id := out[0].Cmd["id"]
	pendingAnswer(t, m, out, []any{map[string]any{"id": id, "type": "follow_up", "message": "then rename it"}})
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	feed(m, jsonLine(map[string]any{"type": "run_start", "id": id, "task": "then rename it"}))
	feed(m, ask("q2", id))
	if m.Modal == nil || m.Modal.For != "the follow-up: then rename it" {
		t.Fatalf("modal %+v", m.Modal)
	}
	key(m, 'n')

	feed(m, ask("q3", "other"))
	if m.Modal == nil || m.Modal.For != "the prompt or follow-up sent as other" {
		t.Fatalf("modal %+v", m.Modal)
	}
	key(m, 'n')
	feed(m, ask("q4", nil))
	if m.Modal == nil || m.Modal.For != "a prompt or follow-up sent with no id" {
		t.Fatalf("modal %+v", m.Modal)
	}
}
