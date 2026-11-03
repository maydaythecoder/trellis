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

// echoWords is the echo of a line classify has not answered: the
// person's, and not yet a task, since it may be a question.
func echoWords(text string) string { return "you  " + view.Plain(text) }

// taskWords is the person's line once trellis has taken it as a task.
func taskWords(text string) string { return "you · task  " + view.Plain(text) }

// route asks trellis what a line is before anything runs.
func (m *Model) route(text string) {
	m.pushYou(echoWords(text))
	m.echoAt = len(m.Transcript)
	m.classifying = true
	m.classifyText = text
	m.ask("classify", forClassify, map[string]any{"text": text})
}

// classified acts on trellis's answer: a question's answer is shown as
// text and nothing runs; a task becomes prompt with the same text.
func (m *Model) classified(r *frames.ResponseFields, ok bool) {
	text := m.classifyText
	echo := m.echoAt
	m.classifying = false
	m.classifyText = ""
	m.echoAt = 0
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
		// Labelled a task only on trellis's word; an echo the cap dropped
		// meanwhile has nothing left to relabel.
		if echo > 0 {
			m.Transcript[echo-1].Text = capLine(taskWords(text))
		}
		m.startRun(text)
	default:
		m.push(view.RoleRefusal, "! trellis answered classify with no intent this view knows; nothing was run")
	}
}
