package model

import (
	"strings"
	"testing"
)

// promptsIn is every prompt in out.
func promptsIn(out []Out) []Out {
	var p []Out
	for _, o := range out {
		if o.Cmd != nil && o.Cmd["type"] == "prompt" {
			p = append(p, o)
		}
	}
	return p
}

// onePrompt is the one prompt out holds, failing unless there is exactly
// one.
func onePrompt(t *testing.T, out []Out, why string) Out {
	t.Helper()
	p := promptsIn(out)
	if len(p) != 1 {
		t.Fatalf("%s: %d prompts in %v, want one", why, len(p), kinds(out))
	}
	return p[0]
}

// #952: a prompt that names no control mode runs in the repository's,
// which trellis reports to no client, so the person could not know
// whether edits would be asked about. Every prompt names one, as the
// desktop's do: approve by default, auto with --control auto, and the
// mode /control set; the prompt a classify settled as a task and a Stop
// and send's held line both carry it, since every prompt goes through
// startRun. /control with a word other than approve or auto is refused
// and changes nothing, and mid-run it is refused with every slash command,
// so a run's mode never changes under it.
func TestControlModeIsSentWithThePrompt(t *testing.T) {
	m := fresh(t)
	submit(m, "fix the bug")
	if p := onePrompt(t, m.TakeOutbox(), "default"); p.Cmd["control"] != Approve {
		t.Fatalf("with no --control the prompt names %v, want approve: %+v", p.Cmd["control"], p.Cmd)
	}

	a := New(Options{Control: Auto, Task: "make f return 2"})
	out := a.TakeOutbox()
	var classify Out
	for _, o := range out {
		if o.Cmd["type"] == "classify" {
			classify = o
		}
	}
	if classify.Cmd == nil {
		t.Fatalf("the start-up task sent no classify: %v", kinds(out))
	}
	respond(a, classify.Cmd["id"], "classify", map[string]any{"intent": "task"})
	if p := onePrompt(t, a.TakeOutbox(), "--control auto, a classified task"); p.Cmd["control"] != Auto {
		t.Fatalf("--control auto sent %v", p.Cmd["control"])
	}

	c := fresh(t)
	for _, bad := range []string{"/control yolo", "/control auto approve"} {
		typeLine(c, bad)
		enter(c)
		if got := lines(c); !strings.Contains(got, "/control takes approve or auto") {
			t.Fatalf("%q was not refused:\n%s", bad, got)
		}
	}
	if out := c.TakeOutbox(); len(out) != 0 || c.control != Approve {
		t.Fatalf("a refused /control sent %v or changed the mode to %q", kinds(out), c.control)
	}
	typeLine(c, "/control auto")
	enter(c)
	if got := last(c); got != "· control auto: the next prompt names it." {
		t.Fatalf("/control auto said %q", got)
	}
	submit(c, "first task")
	if p := onePrompt(t, c.TakeOutbox(), "after /control auto"); p.Cmd["control"] != Auto {
		t.Fatalf("after /control auto the prompt names %v", p.Cmd["control"])
	}
	typeLine(c, "/control approve")
	enter(c)
	if out := c.TakeOutbox(); len(out) != 0 || c.control != Auto {
		t.Fatalf("/control mid-run sent %v or changed the mode to %q", kinds(out), c.control)
	}
	c.Input = ""

	// Stop and send: the held line's prompt names the mode too.
	typeLine(c, "start over")
	tabKey(c)
	tabKey(c)
	enter(c)
	abort := c.TakeOutbox()
	if got := strings.Join(kinds(abort), ","); got != "abort" {
		t.Fatalf("Stop and send sent %s", got)
	}
	feed(c, runEnd(map[string]any{"outcome": "interrupted", "restored": true}))
	respond(c, abort[0].Cmd["id"], "abort", map[string]any{"undelivered": []any{}})
	if p := onePrompt(t, c.TakeOutbox(), "Stop and send's held line"); p.Cmd["control"] != Auto || p.Cmd["task"] != "start over" {
		t.Fatalf("the held line's prompt: %+v", p.Cmd)
	}
}

// #952: Where it is said control —, and named neither the model, the tier
// nor the call the run last made, so a person reading it could not tell
// what was driving the run, how, or what it had just done. It shows the
// mode the prompt named, beside the repository; the model and tier
// get_state named, on one row as the status bar has them, the tier whole
// and the model cut to the room left; and the last turn's tool and target,
// kept once the next model call starts and after the run ends. Each is —
// until it is known, never a guess, and a new run starts with no last call.
func TestWhereItIsShowsControlModelTierAndLastCall(t *testing.T) {
	m := fresh(t)
	m.Update(windowSize(120, 40))
	v := m.View()
	for _, want := range []string{"root     — · control approve", "call     —", "model    — · tier —"} {
		if !strings.Contains(v, want) {
			t.Fatalf("before any prompt %q is not in Where it is:\n%s", want, v)
		}
	}
	typeLine(m, "/control auto")
	enter(m)
	submit(m, "change f")
	m.TakeOutbox()
	state := func(data map[string]any) {
		m.refreshState()
		for _, o := range m.TakeOutbox() {
			if o.Cmd != nil && o.Cmd["type"] == "get_state" {
				respond(m, o.Cmd["id"], "get_state", data)
			}
		}
	}
	state(map[string]any{"model": "ollama:qwen", "tier": "2", "root": "/work/demo"})
	feed(m, modelCall(1))
	feed(m, turn(1, "edit app/calc.py", map[string]any{"tool": "edit", "target": "app/calc.py", "wrote": true}))
	feed(m, modelCall(2))
	v = m.View()
	for _, want := range []string{"phase    calling the model, turn 2", "call     edit → app/calc.py", "model    ollama:qwen · tier 2", "root     demo · control auto"} {
		if !strings.Contains(v, want) {
			t.Fatalf("%q is not in Where it is:\n%s", want, v)
		}
	}
	state(map[string]any{"model": "ollama:qwen2.5-coder:32b", "tier": "2, native tool calls", "root": "/work/demo"})
	if v := m.View(); !strings.Contains(v, "model    ollama… · tier 2, native tool calls│") {
		t.Fatalf("a long model and tier: the tier is not whole or the model not cut:\n%s", v)
	}
	feed(m, runEnd(map[string]any{"outcome": "converged", "exit": 0}))
	if v := m.View(); !strings.Contains(v, "call     edit → app/calc.py") || !strings.Contains(v, "root     demo · control auto") {
		t.Fatalf("after run_end Where it is lost the last call or the mode:\n%s", v)
	}
	submit(m, "check it")
	m.TakeOutbox()
	if v := m.View(); !strings.Contains(v, "call     —") {
		t.Fatalf("a new run kept the last run's call:\n%s", v)
	}
	// A call trellis named no target for is the tool alone.
	feed(m, turn(1, "run_checks", map[string]any{"tool": "run_checks"}))
	if v := m.View(); !strings.Contains(v, "call     run_checks ") || strings.Contains(v, "run_checks →") {
		t.Fatalf("a call with no target:\n%s", v)
	}
}
