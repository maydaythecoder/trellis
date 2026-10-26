package model

import (
	"strings"

	"github.com/maydaythecoder/trellis/tui/internal/frames"
	"github.com/maydaythecoder/trellis/tui/internal/view"
)

// Routing. Whether a typed line is a question or a task is trellis's rule
// (ask::classify), asked through rpc classify (R1, #501), so it has one
// home and this client holds no word list of its own (ADR 0052 decision
// 2.3). Typing `hello` once took a git snapshot, drove the loop, ran the
// checks, rolled back and wrote a trail (tests/hygiene.rs); a question is
// now answered in the transcript and nothing runs.

const forClassify = "classify"

// route asks trellis what a line is before anything runs.
func (m *Model) route(text string) {
	m.push(view.RoleYou, "> "+view.Plain(text))
	m.classifying = true
	m.classifyText = text
	m.ask("classify", forClassify, map[string]any{"text": text})
}

// classified acts on trellis's answer: a question's answer is shown as
// text and nothing runs; a task becomes prompt with the same text.
func (m *Model) classified(r *frames.ResponseFields, ok bool) {
	text := m.classifyText
	m.classifying = false
	m.classifyText = ""
	if m.Quitting {
		return
	}
	if !ok {
		// As tui_run.rs reports a classify error: every line marked, and
		// input handed back, rather than falling through to a run that
		// would fail the same way a moment later.
		why := ""
		if r.Error != nil {
			why = *r.Error
		}
		said := false
		for _, l := range strings.Split(why, "\n") {
			if strings.TrimSpace(l) == "" {
				continue
			}
			m.push(view.RoleRefusal, "! "+view.Plain(l))
			said = true
		}
		if !said {
			m.push(view.RoleRefusal, "! trellis could not tell whether that was a question or a task, and gave no reason")
		}
		m.push(view.RoleNote, "· nothing was run")
		return
	}
	d, _ := r.Data.(map[string]any)
	switch d["intent"] {
	case "question":
		answer, _ := d["answer"].(string)
		if strings.TrimSpace(answer) == "" {
			m.push(view.RoleNote, "· trellis took that for a question and gave no answer; nothing was run")
			return
		}
		m.pushText(view.RoleText, answer)
	case "task":
		m.startRun(text)
	default:
		m.push(view.RoleRefusal, "! trellis answered classify with no intent this view knows; nothing was run")
	}
}
